//! Phase 2 replay recall 評測。
//!
//! 問題與正解是另一份可人工審查的 JSON，不塞回 corpus。Runner 每個配置都把
//! 同一份 corpus 匯進一顆新的記憶體資料庫，走 [`crate::retrieval`] 的正式產品
//! 接線；沒有模型、沒有網路，也不碰使用者真正的資料庫。

use std::collections::BTreeSet;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::Db;
use crate::db::QueryRow;
use crate::facts::extract;
use crate::model::{Millis, SourceKind};
use crate::replay::{Corpus, Event, ReviewStatus};
use crate::retrieval::{Retrieval, RetrievalProfile};

pub const QUESTION_SET_VERSION: u32 = 1;
pub const REPORT_VERSION: u32 = 1;
const EVAL_ORIGIN: Millis = 1_700_000_000_000;
const WARMUPS: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSet {
    pub format_version: u32,
    pub name: String,
    /// 真實 query log 即使搭配 Reviewed corpus，仍然要自己經過人工審查。
    pub review: ReviewStatus,
    /// 題目裡的 event_index 只能套在這一份去敏後 corpus 上。
    pub corpus_fingerprint: String,
    pub questions: Vec<RecallQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallQuestion {
    pub id: String,
    pub question: String,
    pub source: QuestionSource,
    /// query log 匯出時保留相對時間，不帶真實 epoch；手標／埋題可省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at_ms: Option<Millis>,
    /// 舊產品當時怎麼回，只是標註提示，不是 ground truth。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<QueryObservation>,
    /// `null` 是尚未人工標註；絕不把當時 `hits = 0` 猜成 NoAnswer。
    pub expected: Option<ExpectedOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryObservation {
    pub shape: String,
    pub product_results: usize,
    pub interface: String,
    pub opened_sources: usize,
    pub marked_forgotten: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionSource {
    QueryLog,
    HandLabeled,
    Planted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedOutcome {
    Answer {
        /// 任一字串命中就算答對。可同時列原文與 L1 normalized 值。
        any_of: Vec<String>,
        /// v1 每題指定一個 canonical event；存 array index，不存 import-local row id。
        evidence: Vec<EvidenceRef>,
    },
    NoAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub event_index: usize,
}

impl QuestionSet {
    /// 把固定時間窗裡的真實問法變成可人工標註的私有 Draft。
    pub fn draft_from_query_log(
        name: &str,
        corpus: &Corpus,
        origin: Millis,
        rows: &[QueryRow],
    ) -> Result<Self> {
        corpus.validate()?;
        ensure!(!name.trim().is_empty(), "question set 缺少名稱");
        ensure!(!rows.is_empty(), "這段時間裡沒有可匯出的 query log");

        let questions = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let asked_at_ms = row
                    .ts
                    .checked_sub(origin)
                    .with_context(|| format!("query #{} timestamp overflowed", row.id))?;
                ensure!(
                    (0..=corpus.duration_ms).contains(&asked_at_ms),
                    "query #{} 不在 corpus 的時間範圍內",
                    row.id
                );
                Ok(RecallQuestion {
                    id: format!("query-{:04}", index + 1),
                    question: row.question.clone(),
                    source: QuestionSource::QueryLog,
                    asked_at_ms: Some(asked_at_ms),
                    observed: Some(QueryObservation {
                        shape: row.shape.clone(),
                        product_results: usize::try_from(row.hits)
                            .with_context(|| format!("query #{} 的 hits 是負數", row.id))?,
                        interface: row.source.clone(),
                        opened_sources: usize::try_from(row.clicks)
                            .with_context(|| format!("query #{} 的 clicks 是負數", row.id))?,
                        marked_forgotten: row.marked(),
                    }),
                    expected: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let set = Self {
            format_version: QUESTION_SET_VERSION,
            name: name.trim().to_string(),
            review: ReviewStatus::Draft,
            corpus_fingerprint: corpus_fingerprint(corpus)?,
            questions,
        };
        set.validate(corpus)?;
        Ok(set)
    }

    pub fn validate(&self, corpus: &Corpus) -> Result<()> {
        ensure!(
            self.format_version == QUESTION_SET_VERSION,
            "不支援 question set format_version {}（這版只支援 {}）",
            self.format_version,
            QUESTION_SET_VERSION
        );
        ensure!(!self.name.trim().is_empty(), "question set 缺少名稱");
        ensure!(!self.questions.is_empty(), "question set 一題都沒有");
        let expected_fingerprint = corpus_fingerprint(corpus)?;
        ensure!(
            self.corpus_fingerprint == expected_fingerprint,
            "question set 綁定的 corpus fingerprint 不同：檔案是 {}，corpus 是 {}",
            self.corpus_fingerprint,
            expected_fingerprint
        );

        let mut ids = BTreeSet::new();
        for (question_index, question) in self.questions.iter().enumerate() {
            ensure!(
                !question.id.trim().is_empty(),
                "question #{question_index} 缺少 id"
            );
            ensure!(
                ids.insert(question.id.as_str()),
                "question id 重複：{}",
                question.id
            );
            ensure!(
                !question.question.trim().is_empty(),
                "question {} 的問題是空的",
                question.id
            );

            if question.source == QuestionSource::QueryLog {
                let asked_at_ms = question.asked_at_ms.with_context(|| {
                    format!("query-log question {} 缺少 asked_at_ms", question.id)
                })?;
                ensure!(
                    (0..=corpus.duration_ms).contains(&asked_at_ms),
                    "query-log question {} 的 asked_at_ms 不在 corpus 裡",
                    question.id
                );
                let observed = question
                    .observed
                    .as_ref()
                    .with_context(|| format!("query-log question {} 缺少 observed", question.id))?;
                ensure!(
                    matches!(observed.shape.as_str(), "recent" | "keywords"),
                    "query-log question {} 的 observed.shape 不認得：{}",
                    question.id,
                    observed.shape
                );
                ensure!(
                    !observed.interface.trim().is_empty(),
                    "query-log question {} 的 observed.interface 是空的",
                    question.id
                );
            }

            if let Some(ExpectedOutcome::Answer { any_of, evidence }) = &question.expected {
                ensure!(
                    !any_of.is_empty() && any_of.iter().all(|answer| !answer.trim().is_empty()),
                    "question {} 的 answer.any_of 不可為空",
                    question.id
                );
                ensure!(
                    evidence.len() == 1,
                    "question {} 的 v1 answer 必須剛好指定一個 canonical evidence event",
                    question.id,
                );

                for reference in evidence {
                    let event = corpus.events.get(reference.event_index).with_context(|| {
                        format!(
                            "question {} 指到不存在的 corpus event #{}",
                            question.id, reference.event_index
                        )
                    })?;
                    let surfaces = evidence_surfaces(event);
                    ensure!(
                        !surfaces.is_empty(),
                        "question {} 的 evidence event #{} 不會產生可檢索文字",
                        question.id,
                        reference.event_index
                    );
                    let label_reaches_evidence = surfaces.iter().any(|(_, text)| {
                        any_of.iter().any(|answer| text_matches(text, answer))
                            || extract(text).iter().any(|fact| {
                                any_of.iter().any(|answer| {
                                    text_matches(&fact.raw, answer)
                                        || text_matches(&fact.normalized, answer)
                                })
                            })
                    });
                    ensure!(
                        label_reaches_evidence,
                        "question {} 的 any_of 沒有一個出現在 evidence event #{} 裡",
                        question.id,
                        reference.event_index
                    );
                }
            }
            if self.review == ReviewStatus::Reviewed {
                ensure!(
                    question.expected.is_some(),
                    "Reviewed question set 裡仍有未標註題目：{}",
                    question.id
                );
            }
        }
        Ok(())
    }

    /// 回傳一份只改指定題目的 Draft；原本的題庫永遠不會被部分修改。
    pub fn with_answer(
        &self,
        corpus: &Corpus,
        id: &str,
        event_index: usize,
        any_of: Vec<String>,
        replace: bool,
    ) -> Result<Self> {
        self.with_expected(
            corpus,
            id,
            ExpectedOutcome::Answer {
                any_of,
                evidence: vec![EvidenceRef { event_index }],
            },
            replace,
        )
    }

    /// `NoAnswer` 只能由人明確標下；不會從舊產品回傳 0 筆自動推論。
    pub fn with_no_answer(&self, corpus: &Corpus, id: &str, replace: bool) -> Result<Self> {
        self.with_expected(corpus, id, ExpectedOutcome::NoAnswer, replace)
    }

    fn with_expected(
        &self,
        corpus: &Corpus,
        id: &str,
        expected: ExpectedOutcome,
        replace: bool,
    ) -> Result<Self> {
        self.validate(corpus)?;
        ensure!(
            self.review == ReviewStatus::Draft,
            "Reviewed question set 不可直接改標註；請從審查前的 Draft 產生新版"
        );

        let mut next = self.clone();
        let question = next
            .questions
            .iter_mut()
            .find(|question| question.id == id)
            .with_context(|| format!("question id 不存在：{id}"))?;
        ensure!(
            replace || question.expected.is_none(),
            "question {id} 已經有標註；要更正請明確選擇重新標註"
        );
        question.expected = Some(expected);
        next.validate(corpus)?;
        Ok(next)
    }

    /// 全部標完且每個 evidence 都有效，才產生另一份 Reviewed 題庫。
    pub fn reviewed(&self, corpus: &Corpus) -> Result<Self> {
        self.validate(corpus)?;
        ensure!(
            self.review == ReviewStatus::Draft,
            "question set 已經是 Reviewed"
        );
        let missing: Vec<_> = self
            .questions
            .iter()
            .filter(|question| question.expected.is_none())
            .map(|question| question.id.as_str())
            .collect();
        ensure!(
            missing.is_empty(),
            "還有 {} 題沒有標註：{}",
            missing.len(),
            missing.join("、")
        );

        let mut reviewed = self.clone();
        reviewed.review = ReviewStatus::Reviewed;
        reviewed.validate(corpus)?;
        Ok(reviewed)
    }

    fn source_counts(&self) -> QuestionSourceCounts {
        let mut counts = QuestionSourceCounts::default();
        for question in &self.questions {
            match question.source {
                QuestionSource::QueryLog => counts.query_log += 1,
                QuestionSource::HandLabeled => counts.hand_labeled += 1,
                QuestionSource::Planted => counts.planted += 1,
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionSourceCounts {
    pub query_log: usize,
    pub hand_labeled: usize,
    pub planted: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalReport {
    pub format_version: u32,
    pub evaluator_version: String,
    pub corpus: EvaluatedCorpus,
    pub question_set: EvaluatedQuestions,
    pub parameters: EvalParameters,
    pub configurations: Vec<ConfigurationReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedCorpus {
    pub name: String,
    pub format_version: u32,
    pub review: ReviewStatus,
    pub duration_ms: Millis,
    pub events: usize,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedQuestions {
    pub name: String,
    pub format_version: u32,
    pub review: ReviewStatus,
    pub questions: usize,
    pub sources: QuestionSourceCounts,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalParameters {
    pub k: usize,
    pub warmups: usize,
    pub runs: usize,
    /// facts 在前、文字結果在後；和產品畫面相同。
    pub ranking: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationReport {
    pub name: String,
    pub metrics: EvalMetrics,
    pub questions: Vec<QuestionResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionResult {
    pub id: String,
    pub question: String,
    pub source: QuestionSource,
    pub answer_correct: bool,
    /// NoAnswer 題不屬於 recall/citation 的分母，所以明講 `null`。
    pub recalled: Option<bool>,
    pub citation_correct: Option<bool>,
    pub latency_median_ms: f64,
    pub returned: Vec<ReturnedItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReturnedItem {
    pub rank: usize,
    pub channel: RetrievalChannel,
    pub at_ms: Millis,
    pub source_kind: String,
    /// fact 同時保留 raw / normalized；文字結果只有完整原文。
    pub values: Vec<String>,
    /// 以相對時間與來源類型接回可攜 corpus；不輸出 SQLite row id。
    pub event_indexes: Vec<usize>,
}

/// 標註畫面採用產品真正的 `facts` 檢索路徑所看到的候選；它們只供提示，
/// 不會自動變成 ground truth。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationPreview {
    pub question_id: String,
    pub returned: Vec<ReturnedItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalChannel {
    Fact,
    Text,
    Recent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalMetrics {
    pub recall_at_k: Fraction,
    pub answer_accuracy: Fraction,
    pub citation_accuracy: Fraction,
    pub latency: LatencyMetrics,
    /// 兩個現行配置都不呼叫模型；這是由路徑定義得出的 0，不是假裝量過價格。
    pub model_calls: usize,
    pub model_usd_per_day: f64,
    pub reminder_false_positive_rate: Option<f64>,
    pub reminder_miss_rate: Option<f64>,
    pub segmentation_f1: Option<f64>,
    pub reviewer_lookup_rate: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub ram_peak_mb: Option<f64>,
    pub battery_percent_per_hour: Option<f64>,
    pub disk_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fraction {
    pub passed: usize,
    pub total: usize,
    /// 沒有分母就是 `null`，不是 0%。
    pub rate: Option<f64>,
}

impl Fraction {
    fn new(passed: usize, total: usize) -> Self {
        Self {
            passed,
            total,
            rate: (total > 0).then_some(passed as f64 / total as f64),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencyMetrics {
    pub samples: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// 同一份 corpus 自動跑 text baseline 與 +facts。除了 latency，報告只由輸入決定。
pub fn evaluate(
    corpus: &Corpus,
    questions: &QuestionSet,
    k: usize,
    runs: usize,
) -> Result<EvalReport> {
    corpus.validate()?;
    questions.validate(corpus)?;
    if let Some(question) = questions
        .questions
        .iter()
        .find(|question| question.expected.is_none())
    {
        anyhow::bail!(
            "question {} 還沒標註 expected；先填 answer/no_answer 才能 evaluate",
            question.id
        );
    }
    ensure!(k > 0, "--k 必須大於 0");
    ensure!(runs > 0, "--runs 必須大於 0");

    let mut configurations = Vec::new();
    for profile in [RetrievalProfile::TextOnly, RetrievalProfile::TextAndFacts] {
        configurations.push(evaluate_profile(corpus, questions, profile, k, runs)?);
    }

    Ok(EvalReport {
        format_version: REPORT_VERSION,
        evaluator_version: crate::VERSION.to_string(),
        corpus: EvaluatedCorpus {
            name: corpus.name.clone(),
            format_version: corpus.format_version,
            review: corpus.review,
            duration_ms: corpus.duration_ms,
            events: corpus.events.len(),
            fingerprint: corpus_fingerprint(corpus)?,
        },
        question_set: EvaluatedQuestions {
            name: questions.name.clone(),
            format_version: questions.format_version,
            review: questions.review,
            questions: questions.questions.len(),
            sources: questions.source_counts(),
            fingerprint: fingerprint(questions)?,
        },
        parameters: EvalParameters {
            k,
            warmups: WARMUPS,
            runs,
            ranking: "facts_then_text".into(),
        },
        configurations,
    })
}

/// 一次匯入 corpus，替整份題庫產生標註提示，避免每題重建一次資料庫。
pub fn annotation_previews(
    corpus: &Corpus,
    questions: &QuestionSet,
    k: usize,
) -> Result<Vec<AnnotationPreview>> {
    corpus.validate()?;
    questions.validate(corpus)?;
    ensure!(k > 0, "--k 必須大於 0");

    let mut db = Db::open_in_memory()?;
    db.import_replay(corpus, EVAL_ORIGIN)?;
    questions
        .questions
        .iter()
        .map(|question| {
            let retrieval = RetrievalProfile::TextAndFacts.retrieve(&db, &question.question, k)?;
            Ok(AnnotationPreview {
                question_id: question.id.clone(),
                returned: returned_items(corpus, retrieval, k)?,
            })
        })
        .collect()
}

fn evaluate_profile(
    corpus: &Corpus,
    questions: &QuestionSet,
    profile: RetrievalProfile,
    k: usize,
    runs: usize,
) -> Result<ConfigurationReport> {
    let mut db = Db::open_in_memory()?;
    db.import_replay(corpus, EVAL_ORIGIN)?;

    // 整份題庫先暖一輪；不把第一題的冷 cache 優勢送給後面的題目。
    for question in &questions.questions {
        profile.retrieve(&db, &question.question, k)?;
    }

    let mut results = Vec::with_capacity(questions.questions.len());
    let mut all_latency = Vec::with_capacity(questions.questions.len() * runs);
    for question in &questions.questions {
        let mut stable_items: Option<Vec<ReturnedItem>> = None;
        let mut latencies = Vec::with_capacity(runs);
        for _ in 0..runs {
            let started = Instant::now();
            let retrieval = profile.retrieve(&db, &question.question, k)?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            latencies.push(elapsed_ms);
            all_latency.push(elapsed_ms);

            let items = returned_items(corpus, retrieval, k)?;
            if let Some(before) = &stable_items {
                ensure!(
                    *before == items,
                    "同一題在同一配置裡回了不同排名：{} / {}",
                    profile.name(),
                    question.id
                );
            } else {
                stable_items = Some(items);
            }
        }

        let items = stable_items.context("evaluation runs produced no result")?;
        let (answer_correct, recalled, citation_correct) = grade(question, &items);
        results.push(QuestionResult {
            id: question.id.clone(),
            question: question.question.clone(),
            source: question.source,
            answer_correct,
            recalled,
            citation_correct,
            latency_median_ms: percentile(&latencies, 0.50),
            returned: items,
        });
    }

    let positive = results
        .iter()
        .filter(|result| result.recalled.is_some())
        .count();
    let recalled = results
        .iter()
        .filter(|result| result.recalled == Some(true))
        .count();
    let cited = results
        .iter()
        .filter(|result| result.citation_correct == Some(true))
        .count();
    let answered = results
        .iter()
        .filter(|result| result.answer_correct)
        .count();
    let latency = LatencyMetrics {
        samples: all_latency.len(),
        p50_ms: percentile(&all_latency, 0.50),
        p95_ms: percentile(&all_latency, 0.95),
        max_ms: percentile(&all_latency, 1.0),
    };

    Ok(ConfigurationReport {
        name: profile.name().to_string(),
        metrics: EvalMetrics {
            recall_at_k: Fraction::new(recalled, positive),
            answer_accuracy: Fraction::new(answered, results.len()),
            citation_accuracy: Fraction::new(cited, positive),
            latency,
            model_calls: 0,
            model_usd_per_day: 0.0,
            reminder_false_positive_rate: None,
            reminder_miss_rate: None,
            segmentation_f1: None,
            reviewer_lookup_rate: None,
            cpu_percent: None,
            ram_peak_mb: None,
            battery_percent_per_hour: None,
            disk_bytes: None,
        },
        questions: results,
    })
}

fn returned_items(corpus: &Corpus, retrieval: Retrieval, k: usize) -> Result<Vec<ReturnedItem>> {
    let mut raw = Vec::new();
    for answer in retrieval.answers {
        let kind = SourceKind::from_str_kind(&answer.latest.source_kind)
            .with_context(|| format!("unknown fact source kind: {}", answer.latest.source_kind))?;
        raw.push((
            RetrievalChannel::Fact,
            answer.latest.ts - EVAL_ORIGIN,
            kind,
            vec![answer.latest.raw, answer.latest.normalized],
        ));
    }
    let hit_channel = if retrieval.shape == crate::question::Shape::Recent {
        RetrievalChannel::Recent
    } else {
        RetrievalChannel::Text
    };
    for hit in retrieval.hits {
        raw.push((
            hit_channel,
            hit.ts - EVAL_ORIGIN,
            hit.source_kind,
            vec![hit.text],
        ));
    }

    raw.into_iter()
        .take(k)
        .enumerate()
        .map(|(rank, (channel, at_ms, source_kind, values))| {
            ensure!(
                (0..=corpus.duration_ms).contains(&at_ms),
                "retrieval result timestamp {at_ms} fell outside corpus"
            );
            let event_indexes: Vec<_> = corpus
                .events
                .iter()
                .enumerate()
                .filter_map(|(index, event)| {
                    (event.at_ms() == at_ms
                        && event_supports_item(event, source_kind, channel, &values))
                    .then_some(index)
                })
                .collect();
            ensure!(
                !event_indexes.is_empty(),
                "retrieval result at {at_ms}ms ({}) cannot be mapped back to its corpus event",
                source_kind.as_str()
            );
            Ok(ReturnedItem {
                rank: rank + 1,
                channel,
                at_ms,
                source_kind: source_kind.as_str().to_string(),
                values,
                event_indexes,
            })
        })
        .collect()
}

fn grade(question: &RecallQuestion, items: &[ReturnedItem]) -> (bool, Option<bool>, Option<bool>) {
    match question
        .expected
        .as_ref()
        .expect("evaluate rejects unlabeled questions before grading")
    {
        ExpectedOutcome::NoAnswer => (items.is_empty(), None, None),
        ExpectedOutcome::Answer { any_of, evidence } => {
            let expected_events: BTreeSet<_> =
                evidence.iter().map(|item| item.event_index).collect();
            let answer = items.iter().any(|item| item_matches(item, any_of));
            let recall = items.iter().any(|item| {
                item.event_indexes
                    .iter()
                    .any(|index| expected_events.contains(index))
            });
            let citation = items.iter().any(|item| {
                item_matches(item, any_of)
                    && item
                        .event_indexes
                        .iter()
                        .any(|index| expected_events.contains(index))
            });
            (answer, Some(recall), Some(citation))
        }
    }
}

fn item_matches(item: &ReturnedItem, expected: &[String]) -> bool {
    item.values
        .iter()
        .any(|value| expected.iter().any(|answer| text_matches(value, answer)))
}

fn text_matches(value: &str, expected: &str) -> bool {
    value.to_lowercase().contains(&expected.to_lowercase())
}

fn event_supports_item(
    event: &Event,
    source_kind: SourceKind,
    channel: RetrievalChannel,
    values: &[String],
) -> bool {
    evidence_surfaces(event)
        .into_iter()
        .filter(|(kind, _)| *kind == source_kind)
        .any(|(_, surface)| match channel {
            RetrievalChannel::Fact => extract(&surface).iter().any(|fact| {
                values
                    .iter()
                    .any(|value| value == &fact.raw || value == &fact.normalized)
            }),
            RetrievalChannel::Text | RetrievalChannel::Recent => {
                values.iter().any(|value| value == &surface)
            }
        })
}

/// 這是 `answer.evidence` 真正會驗證的文字面；標註 UI 和 validator 共用，
/// 避免畫面顯示一種證據、存檔卻按另一套規則拒絕。
pub fn evidence_surfaces(event: &Event) -> Vec<(SourceKind, String)> {
    match event {
        Event::Frame { ocr, .. } => ocr
            .iter()
            .filter(|block| !block.text.trim().is_empty())
            .map(|block| (SourceKind::Ocr, block.text.clone()))
            .collect(),
        Event::Focus { snapshot, .. } => {
            let mut out = Vec::new();
            if let Some(title) = snapshot
                .window_title
                .as_ref()
                .filter(|title| !title.trim().is_empty())
            {
                out.push((SourceKind::WindowTitle, title.clone()));
            }
            if let Some(url) = snapshot.url.as_ref().filter(|url| !url.trim().is_empty()) {
                out.push((SourceKind::Url, url.clone()));
            }
            out
        }
        Event::Clipboard { text, .. } => text
            .as_ref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| vec![(SourceKind::Clipboard, text.clone())])
            .unwrap_or_default(),
        Event::Input { .. } | Event::System { .. } => Vec::new(),
    }
}

fn percentile(values: &[f64], quantile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = (quantile * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn fingerprint(value: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}

pub fn corpus_fingerprint(corpus: &Corpus) -> Result<String> {
    /// `review` 是人在同一份內容上翻的審查狀態；把它算進去，Draft 題庫會在
    /// corpus 人工改成 Reviewed 的那一刻無故失效。
    #[derive(Serialize)]
    struct Content<'a> {
        format_version: u32,
        name: &'a str,
        duration_ms: Millis,
        redactions: &'a crate::replay::RedactionSummary,
        events: &'a [Event],
    }

    fingerprint(&Content {
        format_version: corpus.format_version,
        name: &corpus.name,
        duration_ms: corpus.duration_ms,
        redactions: &corpus.redactions,
        events: &corpus.events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OcrBlock;
    use crate::replay::{FORMAT_VERSION, RedactionSummary, ReplayFocus};

    fn fixture() -> (Corpus, QuestionSet) {
        let corpus = Corpus {
            format_version: FORMAT_VERSION,
            name: "synthetic recall".into(),
            duration_ms: 2_000,
            review: ReviewStatus::Reviewed,
            redactions: RedactionSummary::default(),
            events: vec![
                Event::Frame {
                    at_ms: 500,
                    monitor: 0,
                    width: 800,
                    height: 600,
                    dhash: 1,
                    dup_run: 0,
                    focus: ReplayFocus::default(),
                    ocr: vec![OcrBlock {
                        text: "客服專線 0800-000-123".into(),
                        x: 0,
                        y: 0,
                        w: 300,
                        h: 20,
                        confidence: 1.0,
                    }],
                },
                Event::Frame {
                    at_ms: 1_500,
                    monitor: 0,
                    width: 800,
                    height: 600,
                    dhash: 2,
                    dup_run: 0,
                    focus: ReplayFocus::default(),
                    ocr: vec![OcrBlock {
                        text: "普通工作筆記".into(),
                        x: 0,
                        y: 30,
                        w: 300,
                        h: 20,
                        confidence: 1.0,
                    }],
                },
            ],
        };
        let questions = QuestionSet {
            format_version: QUESTION_SET_VERSION,
            name: "tiny qa".into(),
            review: ReviewStatus::Reviewed,
            corpus_fingerprint: corpus_fingerprint(&corpus).expect("fingerprint"),
            questions: vec![
                RecallQuestion {
                    id: "phone".into(),
                    question: "電話是什麼".into(),
                    source: QuestionSource::Planted,
                    asked_at_ms: None,
                    observed: None,
                    expected: Some(ExpectedOutcome::Answer {
                        any_of: vec!["0800-000-123".into(), "+886800000123".into()],
                        evidence: vec![EvidenceRef { event_index: 0 }],
                    }),
                },
                RecallQuestion {
                    id: "missing".into(),
                    question: "火星會議連結".into(),
                    source: QuestionSource::HandLabeled,
                    asked_at_ms: None,
                    observed: None,
                    expected: Some(ExpectedOutcome::NoAnswer),
                },
            ],
        };
        (corpus, questions)
    }

    #[test]
    fn facts_beats_the_text_baseline_on_a_synonym_and_no_answer_is_scored() {
        let (corpus, questions) = fixture();
        let report = evaluate(&corpus, &questions, 5, 1).expect("evaluate");
        let text = &report.configurations[0];
        let facts = &report.configurations[1];
        assert_eq!(text.name, "baseline_text");
        assert_eq!(text.metrics.answer_accuracy.passed, 1, "only NoAnswer");
        assert_eq!(text.metrics.recall_at_k.passed, 0);
        assert_eq!(facts.metrics.answer_accuracy.passed, 2);
        assert_eq!(facts.metrics.recall_at_k.passed, 1);
        assert_eq!(facts.metrics.citation_accuracy.passed, 1);
        assert!(facts.questions[0].returned[0].event_indexes.contains(&0));
    }

    #[test]
    fn bad_versions_and_evidence_are_rejected_before_a_run() {
        let (corpus, mut questions) = fixture();
        questions.questions[0].expected = Some(ExpectedOutcome::Answer {
            any_of: vec!["0800".into()],
            evidence: vec![EvidenceRef { event_index: 99 }],
        });
        assert!(questions.validate(&corpus).is_err());
        questions.format_version += 1;
        assert!(questions.validate(&corpus).is_err());
    }

    #[test]
    fn v1_rejects_multiple_evidence_events_instead_of_overstating_recall() {
        let (mut corpus, mut questions) = fixture();
        corpus.events.push(Event::Frame {
            at_ms: 2_000,
            monitor: 0,
            width: 800,
            height: 600,
            dhash: 3,
            dup_run: 0,
            focus: ReplayFocus::default(),
            ocr: vec![OcrBlock {
                text: "客服專線 0800-000-123".into(),
                x: 0,
                y: 0,
                w: 300,
                h: 20,
                confidence: 1.0,
            }],
        });
        questions.corpus_fingerprint = corpus_fingerprint(&corpus).expect("fingerprint");
        let Some(ExpectedOutcome::Answer { evidence, .. }) = &mut questions.questions[0].expected
        else {
            unreachable!()
        };
        evidence.push(EvidenceRef { event_index: 2 });
        assert!(questions.validate(&corpus).is_err());
    }

    #[test]
    fn same_millisecond_ocr_results_only_cite_the_event_with_the_returned_text() {
        let (mut corpus, mut questions) = fixture();
        corpus.events.insert(
            1,
            Event::Frame {
                at_ms: 500,
                monitor: 1,
                width: 800,
                height: 600,
                dhash: 9,
                dup_run: 0,
                focus: ReplayFocus::default(),
                ocr: vec![OcrBlock {
                    text: "另一個螢幕".into(),
                    x: 0,
                    y: 0,
                    w: 300,
                    h: 20,
                    confidence: 1.0,
                }],
            },
        );
        questions.corpus_fingerprint = corpus_fingerprint(&corpus).expect("fingerprint");
        let report = evaluate(&corpus, &questions, 5, 1).expect("evaluate");
        let phone = &report.configurations[1].questions[0].returned[0];
        assert_eq!(phone.event_indexes, vec![0]);
    }

    #[test]
    fn metrics_not_measured_by_this_harness_are_null_not_zero() {
        let (corpus, questions) = fixture();
        let report = evaluate(&corpus, &questions, 5, 1).expect("evaluate");
        let json = serde_json::to_string(&report).expect("json");
        for field in [
            "reminder_false_positive_rate",
            "reminder_miss_rate",
            "segmentation_f1",
            "reviewer_lookup_rate",
            "cpu_percent",
            "ram_peak_mb",
            "battery_percent_per_hour",
            "disk_bytes",
        ] {
            assert!(json.contains(&format!("\"{field}\":null")), "{json}");
        }
        assert!(json.contains("\"model_calls\":0"), "{json}");
    }

    #[test]
    fn a_reviewed_corpus_does_not_review_a_private_question_set_for_free() {
        let (corpus, mut questions) = fixture();
        let mut draft_corpus = corpus.clone();
        draft_corpus.review = ReviewStatus::Draft;
        questions
            .validate(&draft_corpus)
            .expect("review flip does not change corpus content fingerprint");
        questions.review = ReviewStatus::Draft;
        let report = evaluate(&corpus, &questions, 5, 1).expect("evaluate");
        assert_eq!(report.corpus.review, ReviewStatus::Reviewed);
        assert_eq!(report.question_set.review, ReviewStatus::Draft);
    }

    #[test]
    fn query_log_becomes_an_unlabeled_private_draft_without_guessing_no_answer() {
        let (corpus, _) = fixture();
        let rows = vec![
            QueryRow {
                id: 41,
                ts: EVAL_ORIGIN + 100,
                question: "同一句".into(),
                shape: "keywords".into(),
                hits: 0,
                latency_ms: 2,
                source: "desktop".into(),
                clicks: 0,
                marked_ts: None,
            },
            QueryRow {
                id: 99,
                ts: EVAL_ORIGIN + 200,
                question: "同一句".into(),
                shape: "keywords".into(),
                hits: 3,
                latency_ms: 1,
                source: "cli".into(),
                clicks: 2,
                marked_ts: Some(EVAL_ORIGIN + 250),
            },
        ];
        let set = QuestionSet::draft_from_query_log("real words", &corpus, EVAL_ORIGIN, &rows)
            .expect("draft");
        assert_eq!(set.review, ReviewStatus::Draft);
        assert_eq!(set.questions[0].id, "query-0001");
        assert_eq!(set.questions[1].id, "query-0002");
        assert_eq!(set.questions[0].question, set.questions[1].question);
        assert!(
            set.questions
                .iter()
                .all(|question| question.expected.is_none())
        );
        assert_eq!(
            set.questions[0].observed.as_ref().unwrap().product_results,
            0,
            "當時空手不是 ground-truth NoAnswer"
        );
        assert!(set.questions[1].observed.as_ref().unwrap().marked_forgotten);
        let error = evaluate(&corpus, &set, 5, 1).unwrap_err().to_string();
        assert!(
            error.contains("query-0001") && error.contains("還沒標註"),
            "{error}"
        );
    }

    #[test]
    fn draft_labels_are_explicit_validated_and_transactional() {
        let (corpus, mut draft) = fixture();
        draft.review = ReviewStatus::Draft;
        for question in &mut draft.questions {
            question.expected = None;
        }
        let original = draft.clone();

        assert!(
            draft
                .with_answer(&corpus, "phone", 0, vec!["火星".into()], false)
                .is_err(),
            "答案不在 evidence 裡不能存"
        );
        assert_eq!(draft, original, "失敗的標註不能部分改到來源物件");

        let one = draft
            .with_answer(&corpus, "phone", 0, vec!["0800-000-123".into()], false)
            .expect("valid answer");
        assert!(
            one.with_no_answer(&corpus, "phone", false).is_err(),
            "已有標註不能默默被蓋掉"
        );
        assert!(matches!(
            one.with_no_answer(&corpus, "phone", true)
                .expect("explicit replacement")
                .questions[0]
                .expected,
            Some(ExpectedOutcome::NoAnswer)
        ));
        let two = one
            .with_no_answer(&corpus, "missing", false)
            .expect("explicit no answer");
        let reviewed = two.reviewed(&corpus).expect("all labels are valid");
        assert_eq!(reviewed.review, ReviewStatus::Reviewed);
        assert_eq!(reviewed.questions[0].expected, two.questions[0].expected);
    }

    #[test]
    fn review_refuses_unlabeled_or_mismatched_inputs() {
        let (corpus, mut draft) = fixture();
        draft.review = ReviewStatus::Draft;
        draft.questions[1].expected = None;
        let error = draft.reviewed(&corpus).unwrap_err().to_string();
        assert!(
            error.contains("missing") && error.contains("沒有標註"),
            "{error}"
        );

        let mut changed = corpus.clone();
        let Event::Frame { ocr, .. } = &mut changed.events[0] else {
            unreachable!()
        };
        ocr[0].text.push('！');
        assert!(
            draft
                .with_no_answer(&changed, "missing", false)
                .unwrap_err()
                .to_string()
                .contains("fingerprint")
        );
    }

    #[test]
    fn annotation_preview_uses_the_real_facts_profile_without_labeling() {
        let (corpus, mut draft) = fixture();
        draft.review = ReviewStatus::Draft;
        for question in &mut draft.questions {
            question.expected = None;
        }
        let before = draft.clone();
        let previews = annotation_previews(&corpus, &draft, 5).expect("previews");
        assert_eq!(previews.len(), 2);
        assert_eq!(previews[0].question_id, "phone");
        assert!(previews[0].returned.iter().any(|item| {
            item.event_indexes.contains(&0)
                && item.values.iter().any(|value| value.contains("0800"))
        }));
        assert!(previews[1].returned.is_empty());
        assert_eq!(draft, before, "提示不能順手把結果猜成 ground truth");
    }
}
