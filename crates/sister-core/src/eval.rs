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
use crate::brain::{self, InterpretInput};
use crate::config::BrainConfig;
use crate::consent::{Consent, Sheet};
use crate::db::{CommitmentRow, QueryRow};
use crate::facts::extract;
use crate::model::{Millis, SourceKind};
use crate::replay::{Corpus, Event, ReviewStatus};
use crate::retrieval::{Retrieval, RetrievalProfile};
use crate::reviewer::{self, ReviewInput, ReviewKind};

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
                    matches!(observed.shape.as_str(), "recent" | "keywords" | "range"),
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
    /// 同一份題庫「不跑腦 vs 跑腦」的並排。沒開 A/B 就是 `null`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab: Option<AbComparison>,
}

/// 評測時要不要真的 spawn 解釋層／審閱層。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainEval {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbComparison {
    pub baseline: String,
    pub treatment: String,
    /// 答案正確率差值，單位是百分點。沒有分母就是 `null`。
    pub accuracy_delta_pt: Option<f64>,
    pub won_accuracy: bool,
    pub false_commitment: Fraction,
    /// 沒有承諾時是 `null`——那是還沒量到，不是過了 <20% 那一關。
    pub false_commitment_ok: Option<bool>,
    pub gate: AbGate,
    pub questions_total: usize,
    pub questions_graded: usize,
    pub questions_skipped: Vec<SkippedQuestion>,
    pub brain: BrainRunSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbGate {
    Won,
    Lost {
        accuracy: bool,
        false_commitments: bool,
    },
    Incomplete {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkippedQuestion {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainRunSummary {
    pub ran: bool,
    pub skip: Option<String>,
    pub interpreter_jobs: usize,
    pub interpreter_success: usize,
    pub reviewer_ran: bool,
    pub reviewer_skip: Option<String>,
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
    /// facts 在前、章節其次、文字結果在後。沒有章節的配置這一格是空的。
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
    /// 章節的核心迄點（半開）。文字／事實沒有範圍，是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<Millis>,
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
    Range,
    Session,
    Hypothesis,
}

/// 模型花費。兩種 0 不准長得一樣。
///
/// `NotOnPath`：這條配置根本沒有模型路徑（不跑腦）。
/// `Measured`：有跑腦，從 `brain_outbound` 數出來。`calls = 0` 是跑了但一次
/// 都沒送出去，不是「沒量」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelUsage {
    NotOnPath,
    Measured {
        calls: usize,
        usd_per_day: f64,
        interpreter_calls: usize,
        reviewer_calls: usize,
        interpreter_unit_usd: f64,
        reviewer_unit_usd: f64,
        /// 單價與檔位的出處。沒有出處的美元數字比沒有數字更糟。
        source: String,
    },
}

/// SPEC.md §13 預設檔位、`research/cost-model.md` 2026-08-17 官方價。
/// Claude Haiku 4.5：$1.00 / MTok input、$5.00 / MTok output。
pub const HAIKU_INPUT_USD_PER_MTOK: f64 = 1.00;
pub const HAIKU_OUTPUT_USD_PER_MTOK: f64 = 5.00;
/// Scenario B 解釋層中位：4k in / 300 out。
pub const INTERPRETER_INPUT_TOKENS: f64 = 4_000.0;
pub const INTERPRETER_OUTPUT_TOKENS: f64 = 300.0;
/// Scenario B 審閱層中位：8k in / 500 out。
pub const REVIEWER_INPUT_TOKENS: f64 = 8_000.0;
pub const REVIEWER_OUTPUT_TOKENS: f64 = 500.0;
/// 答案正確率要贏 baseline 多少個百分點才算過 A/B 閘門。
pub const AB_ACCURACY_WIN_PT: f64 = 10.0;
/// 誤承諾率上限（分母是 Reviewer 寫進 L3 的承諾數）。
pub const AB_FALSE_COMMITMENT_MAX: f64 = 0.20;

pub const MODEL_PRICE_SOURCE: &str = concat!(
    "SPEC.md §13 預設檔位；單價取 research/cost-model.md 2026-08-17 ",
    "Claude Haiku 4.5 官方價 $1.00/$5.00 per MTok，用量取同文 Scenario B 中位",
    "（解釋 4k in / 300 out、審閱 8k in / 500 out）。",
    "月費對照：Haiku Scenario B 中位 $13.31/月（8h×22 天），exit criteria < US$15/月。"
);

pub fn interpreter_unit_usd() -> f64 {
    INTERPRETER_INPUT_TOKENS / 1_000_000.0 * HAIKU_INPUT_USD_PER_MTOK
        + INTERPRETER_OUTPUT_TOKENS / 1_000_000.0 * HAIKU_OUTPUT_USD_PER_MTOK
}

pub fn reviewer_unit_usd() -> f64 {
    REVIEWER_INPUT_TOKENS / 1_000_000.0 * HAIKU_INPUT_USD_PER_MTOK
        + REVIEWER_OUTPUT_TOKENS / 1_000_000.0 * HAIKU_OUTPUT_USD_PER_MTOK
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalMetrics {
    pub recall_at_k: Fraction,
    pub answer_accuracy: Fraction,
    pub citation_accuracy: Fraction,
    pub latency: LatencyMetrics,
    /// 沒跑腦是 `not_on_path`，不是量到 0 次呼叫。跑了但一次都沒送，才是
    /// `measured` 且 `calls = 0`。
    pub model: ModelUsage,
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

/// 開發者指標頁可以看的評測報告。
///
/// 刻意不是 [`EvalReport`] 的別名：完整報告裡有使用者的問題原話與
/// 檢索回來的內文。這個專用投影拿掉所有自由字串，只留下面板會畫的
/// 數值摘要，以及能按 question set 順序找回去的失敗題號。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricsView {
    pub format_version: u32,
    pub private_draft: bool,
    pub corpus: MetricsCorpusView,
    pub question_set: MetricsQuestionSetView,
    pub parameters: MetricsParametersView,
    pub configurations: Vec<MetricsConfigurationView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricsCorpusView {
    pub review: ReviewStatus,
    pub duration_ms: Millis,
    pub events: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricsQuestionSetView {
    pub review: ReviewStatus,
    pub questions: usize,
    pub sources: QuestionSourceCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetricsParametersView {
    pub k: usize,
    pub warmups: usize,
    pub runs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricsConfigurationView {
    pub name: String,
    pub recall_at_k: Fraction,
    pub answer_accuracy: Fraction,
    pub citation_accuracy: Fraction,
    pub latency: LatencyMetrics,
    pub model: ModelUsage,
    pub reminder_false_positive_rate: Option<f64>,
    pub reminder_miss_rate: Option<f64>,
    pub segmentation_f1: Option<f64>,
    pub reviewer_lookup_rate: Option<f64>,
    pub cpu_percent: Option<f64>,
    pub ram_peak_mb: Option<f64>,
    pub battery_percent_per_hour: Option<f64>,
    pub disk_bytes: Option<u64>,
    /// 1-based question set 順序；不用輸入裡可自由填寫的 id，避免 id 自己裝著原問句。
    pub failed_question_numbers: Vec<usize>,
}

/// 讀完整 report JSON，用 strict serde schema 解析後縮成不含自由字串的
/// 開發者面板 view。
pub fn metrics_view_from_json(contents: &str) -> Result<MetricsView> {
    let report: EvalReport =
        serde_json::from_str(contents).context("parse replay evaluation report JSON")?;
    metrics_view(&report)
}

pub fn metrics_view(report: &EvalReport) -> Result<MetricsView> {
    ensure!(
        report.format_version == REPORT_VERSION,
        "eval report format_version {} 不支援（現行是 {}）",
        report.format_version,
        REPORT_VERSION
    );

    Ok(MetricsView {
        format_version: report.format_version,
        private_draft: report.corpus.review == ReviewStatus::Draft
            || report.question_set.review == ReviewStatus::Draft,
        corpus: MetricsCorpusView {
            review: report.corpus.review,
            duration_ms: report.corpus.duration_ms,
            events: report.corpus.events,
        },
        question_set: MetricsQuestionSetView {
            review: report.question_set.review,
            questions: report.question_set.questions,
            sources: report.question_set.sources.clone(),
        },
        parameters: MetricsParametersView {
            k: report.parameters.k,
            warmups: report.parameters.warmups,
            runs: report.parameters.runs,
        },
        configurations: report
            .configurations
            .iter()
            .enumerate()
            .map(|(configuration_index, configuration)| {
                let metrics = &configuration.metrics;
                MetricsConfigurationView {
                    name: match configuration.name.as_str() {
                        "baseline_text" => "baseline_text".to_string(),
                        "facts" => "facts".to_string(),
                        "facts_session" => "facts_session".to_string(),
                        "interpreter_reviewer" => "interpreter_reviewer".to_string(),
                        _ => format!("configuration-{}", configuration_index + 1),
                    },
                    recall_at_k: metrics.recall_at_k.clone(),
                    answer_accuracy: metrics.answer_accuracy.clone(),
                    citation_accuracy: metrics.citation_accuracy.clone(),
                    latency: metrics.latency.clone(),
                    model: metrics.model.clone(),
                    reminder_false_positive_rate: metrics.reminder_false_positive_rate,
                    reminder_miss_rate: metrics.reminder_miss_rate,
                    segmentation_f1: metrics.segmentation_f1,
                    reviewer_lookup_rate: metrics.reviewer_lookup_rate,
                    cpu_percent: metrics.cpu_percent,
                    ram_peak_mb: metrics.ram_peak_mb,
                    battery_percent_per_hour: metrics.battery_percent_per_hour,
                    disk_bytes: metrics.disk_bytes,
                    failed_question_numbers: configuration
                        .questions
                        .iter()
                        .enumerate()
                        .filter_map(|(question_index, question)| {
                            (!question.answer_correct
                                || question.recalled == Some(false)
                                || question.citation_correct == Some(false))
                            .then_some(question_index + 1)
                        })
                        .collect(),
                }
            })
            .collect(),
    })
}

/// 同一份 corpus 自動跑 text baseline、+facts、+facts+session。除了 latency，報告只由輸入決定。
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
    for profile in [
        RetrievalProfile::TextOnly,
        RetrievalProfile::TextAndFacts,
        RetrievalProfile::TextFactsAndSession,
    ] {
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
            ranking: "facts_then_session_then_text".into(),
        },
        configurations,
        ab: None,
    })
}

/// 同一份題庫跑不跑腦兩路。`brain` 是 `None` 時解釋層會記成「沒設定命令」而跳過，
/// 題庫仍全跑，不會縮小。
pub fn evaluate_ab(
    corpus: &Corpus,
    questions: &QuestionSet,
    k: usize,
    runs: usize,
    brain: Option<&BrainEval>,
) -> Result<EvalReport> {
    let mut report = evaluate(corpus, questions, k, runs)?;
    let origin = replay_origin();
    let mut db = Db::open_in_memory()?;
    db.import_replay(corpus, origin)?;

    let (brain_summary, model) = run_brain_for_eval(&mut db, corpus, origin, brain)?;
    let mut treatment = evaluate_on_db(
        &mut db,
        corpus,
        questions,
        RetrievalProfile::TextFactsAndSession,
        OnDbOpts {
            k,
            runs,
            origin,
            model,
            include_l2: true,
        },
    )?;
    treatment.name = "interpreter_reviewer".into();

    let baseline = report
        .configurations
        .iter()
        .find(|c| c.name == "facts_session")
        .context("A/B 找不到 facts_session 當 baseline")?;
    let false_commitment = false_commitment_rate(&db)?;
    let ab = compare_ab(
        &baseline.metrics.answer_accuracy,
        &treatment.metrics.answer_accuracy,
        &false_commitment,
        questions.questions.len(),
        questions.questions.len(),
        Vec::new(),
        brain_summary,
    );
    report.ab = Some(ab);
    report.configurations.push(treatment);
    Ok(report)
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

    let origin = replay_origin();
    let mut db = Db::open_in_memory()?;
    db.import_replay(corpus, origin)?;
    questions
        .questions
        .iter()
        .map(|question| {
            let now = query_now(question, origin, corpus.duration_ms);
            let retrieval = RetrievalProfile::TextAndFacts.retrieve_at(
                &mut db,
                &question.question,
                crate::retrieval::RetrievalLimits::same(k),
                now,
            )?;
            Ok(AnnotationPreview {
                question_id: question.id.clone(),
                returned: returned_items(corpus, origin, retrieval, k)?,
            })
        })
        .collect()
}

/// 語料 t=0 對在本地昨天 13:00，讓「昨天下午」在任何時區都能蓋到從 t=0 起的那幾個小時。
fn replay_origin() -> Millis {
    use chrono::{Local, TimeZone};
    let today = Local::now().date_naive();
    let Some(yesterday) = today.pred_opt() else {
        return EVAL_ORIGIN;
    };
    let Some(naive) = yesterday.and_hms_opt(13, 0, 0) else {
        return EVAL_ORIGIN;
    };
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            dt.timestamp_millis()
        }
        chrono::LocalResult::None => EVAL_ORIGIN,
    }
}

fn query_now(question: &RecallQuestion, origin: Millis, duration_ms: Millis) -> Millis {
    origin + question.asked_at_ms.unwrap_or(duration_ms)
}

fn evaluate_profile(
    corpus: &Corpus,
    questions: &QuestionSet,
    profile: RetrievalProfile,
    k: usize,
    runs: usize,
) -> Result<ConfigurationReport> {
    let origin = replay_origin();
    let mut db = Db::open_in_memory()?;
    db.import_replay(corpus, origin)?;
    evaluate_on_db(
        &mut db,
        corpus,
        questions,
        profile,
        OnDbOpts {
            k,
            runs,
            origin,
            model: ModelUsage::NotOnPath,
            include_l2: false,
        },
    )
}

struct OnDbOpts {
    k: usize,
    runs: usize,
    origin: Millis,
    model: ModelUsage,
    include_l2: bool,
}

fn evaluate_on_db(
    db: &mut Db,
    corpus: &Corpus,
    questions: &QuestionSet,
    profile: RetrievalProfile,
    opts: OnDbOpts,
) -> Result<ConfigurationReport> {
    let OnDbOpts {
        k,
        runs,
        origin,
        model,
        include_l2,
    } = opts;
    // 整份題庫先暖一輪；不把第一題的冷 cache 優勢送給後面的題目。
    for question in &questions.questions {
        let now = query_now(question, origin, corpus.duration_ms);
        profile.retrieve_at(
            db,
            &question.question,
            crate::retrieval::RetrievalLimits::same(k),
            now,
        )?;
    }

    let mut results = Vec::with_capacity(questions.questions.len());
    let mut all_latency = Vec::with_capacity(questions.questions.len() * runs);
    for question in &questions.questions {
        let mut stable_items: Option<Vec<ReturnedItem>> = None;
        let mut latencies = Vec::with_capacity(runs);
        let now = query_now(question, origin, corpus.duration_ms);
        for _ in 0..runs {
            let started = Instant::now();
            let retrieval = profile.retrieve_at(
                db,
                &question.question,
                crate::retrieval::RetrievalLimits::same(k),
                now,
            )?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            latencies.push(elapsed_ms);
            all_latency.push(elapsed_ms);

            let mut items = returned_items(corpus, origin, retrieval, k)?;
            if include_l2 {
                append_hypothesis_items(db, corpus, origin, &mut items, k)?;
            }
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
            model,
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

fn append_hypothesis_items(
    db: &Db,
    corpus: &Corpus,
    origin: Millis,
    items: &mut Vec<ReturnedItem>,
    k: usize,
) -> Result<()> {
    if items.len() >= k {
        return Ok(());
    }
    let cards = db.l2_in_range(origin, origin + corpus.duration_ms)?;
    let extra = hypothesis_items(corpus, origin, &cards)?;
    for item in extra {
        if items.len() >= k {
            break;
        }
        if items
            .iter()
            .any(|have| have.event_indexes == item.event_indexes && have.values == item.values)
        {
            continue;
        }
        items.push(item);
    }
    for (rank, item) in items.iter_mut().enumerate() {
        item.rank = rank + 1;
    }
    Ok(())
}

fn hypothesis_items(
    corpus: &Corpus,
    origin: Millis,
    cards: &[crate::db::L2CardRow],
) -> Result<Vec<ReturnedItem>> {
    let mut latest: std::collections::BTreeMap<Millis, &crate::db::L2CardRow> =
        std::collections::BTreeMap::new();
    for card in cards {
        latest
            .entry(card.segment_core_start)
            .and_modify(|cur| {
                if card.version > cur.version || (card.version == cur.version && card.id > cur.id) {
                    *cur = card;
                }
            })
            .or_insert(card);
    }
    let mut out = Vec::new();
    for card in latest.into_values() {
        let at_ms = card.segment_core_start - origin;
        let mut values = vec![card.activity.clone()];
        if let Ok(entities) = serde_json::from_str::<Vec<brain::Entity>>(&card.entities_json) {
            for entity in entities {
                if !entity.name.trim().is_empty() {
                    values.push(entity.name);
                }
            }
        }
        let event_indexes: Vec<_> = corpus
            .events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                (event.at_ms() == at_ms && !super_evidence_empty(event)).then_some(index)
            })
            .collect();
        if event_indexes.is_empty() {
            continue;
        }
        out.push(ReturnedItem {
            rank: 0,
            channel: RetrievalChannel::Hypothesis,
            at_ms,
            end_ms: None,
            source_kind: SourceKind::Ocr.as_str().to_string(),
            values,
            event_indexes,
        });
    }
    Ok(out)
}

fn super_evidence_empty(event: &Event) -> bool {
    evidence_surfaces(event).is_empty()
}

fn run_brain_for_eval(
    db: &mut Db,
    corpus: &Corpus,
    origin: Millis,
    brain: Option<&BrainEval>,
) -> Result<(BrainRunSummary, ModelUsage)> {
    let mut consent = Consent::default();
    consent.grant(Sheet::CloudReading, origin);
    consent.grant(Sheet::LocalRecording, origin);

    let (command, args, configured) = match brain {
        Some(cfg) if !cfg.command.trim().is_empty() => {
            (cfg.command.clone(), cfg.args.clone(), true)
        }
        _ => (String::new(), Vec::new(), false),
    };
    let brain_cfg = BrainConfig {
        command: command.clone(),
        args: args.clone(),
        daily_budget: 80,
        concurrency: 1,
        reviewer_daily_budget: 40,
    };

    let mut summary = BrainRunSummary {
        ran: false,
        skip: None,
        interpreter_jobs: 0,
        interpreter_success: 0,
        reviewer_ran: false,
        reviewer_skip: None,
    };

    if !configured {
        summary.skip = Some("no_command".into());
        return Ok((summary, ModelUsage::NotOnPath));
    }

    let from_ts = origin;
    let to_ts = origin
        .checked_add(corpus.duration_ms)
        .context("eval brain range overflowed")?;
    let interpreted = {
        let mut input = InterpretInput {
            db,
            consent: &consent,
            brain: &brain_cfg,
            from_ts,
            to_ts,
            limit: 80,
            only_core_start: None,
        };
        brain::run(&mut input)?
    };
    summary.ran = true;
    summary.skip = interpreted.skip.as_ref().map(|s| s.as_str().to_string());
    summary.interpreter_jobs = interpreted.ran.len();
    summary.interpreter_success = interpreted
        .ran
        .iter()
        .filter(|job| job.outcome == brain::OutboundOutcome::Success)
        .count();

    let reviewed = {
        let mut review_input = ReviewInput {
            db,
            consent: &consent,
            brain: &brain_cfg,
            from_ts,
            to_ts,
            kind: ReviewKind::Interval,
            force: true,
            now: to_ts,
        };
        reviewer::run(&mut review_input)?
    };
    summary.reviewer_ran = reviewed.ran;
    summary.reviewer_skip = reviewed.skip.as_ref().map(|s| s.as_str().to_string());

    let outbound = db.list_brain_outbound(10_000)?;
    let interpreter_calls = outbound
        .iter()
        .filter(|row| row.role == "interpreter")
        .count();
    let reviewer_calls = outbound.iter().filter(|row| row.role == "reviewer").count();
    let interp_unit = interpreter_unit_usd();
    let review_unit = reviewer_unit_usd();
    let usd_per_day = interpreter_calls as f64 * interp_unit + reviewer_calls as f64 * review_unit;
    Ok((
        summary,
        ModelUsage::Measured {
            calls: interpreter_calls + reviewer_calls,
            usd_per_day,
            interpreter_calls,
            reviewer_calls,
            interpreter_unit_usd: interp_unit,
            reviewer_unit_usd: review_unit,
            source: MODEL_PRICE_SOURCE.to_string(),
        },
    ))
}

fn false_commitment_rate(db: &Db) -> Result<Fraction> {
    let rows = db.all_commitments()?;
    let live: Vec<&CommitmentRow> = rows
        .iter()
        .filter(|row| row.tombstoned_at.is_none())
        .collect();
    let mut false_n = 0usize;
    for row in &live {
        if row.status == "dead" || commitment_unmatched(db, row)? {
            false_n += 1;
        }
    }
    Ok(Fraction::new(false_n, live.len()))
}

fn commitment_unmatched(db: &Db, row: &CommitmentRow) -> Result<bool> {
    let refs: Vec<String> = serde_json::from_str(&row.evidence_json).unwrap_or_default();
    if refs.is_empty() {
        return Ok(true);
    }
    let mut saw_original = false;
    for raw in &refs {
        let Some(r) = brain::EvidenceRef::parse(raw) else {
            return Ok(true);
        };
        match db.l0_original(&r)? {
            None => return Ok(true),
            Some(original) => {
                saw_original = true;
                if !original.text.contains(row.text.trim()) && !row.text.trim().is_empty() {
                    // 原文對得上才算有證據。對不上 = 幽靈承諾。
                    return Ok(true);
                }
            }
        }
    }
    Ok(!saw_original)
}

fn compare_ab(
    baseline: &Fraction,
    treatment: &Fraction,
    false_commitment: &Fraction,
    questions_total: usize,
    questions_graded: usize,
    questions_skipped: Vec<SkippedQuestion>,
    brain: BrainRunSummary,
) -> AbComparison {
    let accuracy_delta_pt = match (treatment.rate, baseline.rate) {
        (Some(t), Some(b)) => Some((t - b) * 100.0),
        _ => None,
    };
    let won_accuracy = accuracy_delta_pt
        .map(|delta| delta + f64::EPSILON >= AB_ACCURACY_WIN_PT)
        .unwrap_or(false);
    let false_commitment_ok = false_commitment
        .rate
        .map(|rate| rate < AB_FALSE_COMMITMENT_MAX);

    let gate = if !brain.ran && brain.skip.is_some() {
        AbGate::Incomplete {
            reason: brain.skip.clone().unwrap_or_else(|| "brain_skipped".into()),
        }
    } else if false_commitment.total == 0 {
        AbGate::Incomplete {
            reason: "no_commitments".into(),
        }
    } else if accuracy_delta_pt.is_none() {
        AbGate::Incomplete {
            reason: "no_accuracy_denominator".into(),
        }
    } else if won_accuracy && false_commitment_ok == Some(true) {
        AbGate::Won
    } else {
        AbGate::Lost {
            accuracy: won_accuracy,
            false_commitments: false_commitment_ok == Some(true),
        }
    };

    AbComparison {
        baseline: "facts_session".into(),
        treatment: "interpreter_reviewer".into(),
        accuracy_delta_pt,
        won_accuracy,
        false_commitment: false_commitment.clone(),
        false_commitment_ok,
        gate,
        questions_total,
        questions_graded,
        questions_skipped,
        brain,
    }
}

/// 給 CLI 印的那一行。兩種 0 的字不一樣。
pub fn format_model_usage(model: &ModelUsage) -> String {
    match model {
        ModelUsage::NotOnPath => "沒跑腦（不是量到 0 次呼叫）".into(),
        ModelUsage::Measured {
            calls,
            usd_per_day: _,
            interpreter_calls: _,
            reviewer_calls: _,
            interpreter_unit_usd,
            reviewer_unit_usd,
            source,
        } if *calls == 0 => format!(
            "跑了腦，0 次呼叫，US$0/天（單價：解釋 US${interpreter_unit_usd:.4}/次、審閱 US${reviewer_unit_usd:.4}/次。{source}）"
        ),
        ModelUsage::Measured {
            calls,
            usd_per_day,
            interpreter_calls,
            reviewer_calls,
            interpreter_unit_usd,
            reviewer_unit_usd,
            source,
        } => format!(
            "{calls} calls（解釋 {interpreter_calls}、審閱 {reviewer_calls}），US${usd_per_day:.4}/天（單價：解釋 US${interpreter_unit_usd:.4}/次、審閱 US${reviewer_unit_usd:.4}/次。{source}）"
        ),
    }
}

pub fn format_false_commitment(value: &Fraction) -> String {
    match value.rate {
        None => "沒有承諾（誤承諾率還沒量到，不是 0%）".into(),
        Some(rate) => format!("{}/{}（{:.1}%）", value.passed, value.total, rate * 100.0),
    }
}

pub fn format_ab_gate(gate: &AbGate) -> String {
    match gate {
        AbGate::Won => {
            "過了 +10pt 與誤承諾 <20%。產品預設仍然關著——這支工具只報數字，不改設定。".into()
        }
        AbGate::Lost {
            accuracy,
            false_commitments,
        } => {
            let acc = if *accuracy {
                "答案正確率贏了"
            } else {
                "答案正確率沒贏（要 +10pt）"
            };
            let fc = if *false_commitments {
                "誤承諾率過關"
            } else {
                "誤承諾率沒過（要 <20%）"
            };
            format!("沒贏，保持預設關。{acc}；{fc}。")
        }
        AbGate::Incomplete { reason } => match reason.as_str() {
            "no_command" => "沒跑成（還沒設定 CLI），不能宣布贏。保持預設關。".into(),
            "no_consent" => "沒跑成（沒簽同意書），不能宣布贏。保持預設關。".into(),
            "no_commitments" => {
                "沒有承諾，誤承諾率還沒量到（不是 0%）。不能宣布贏。保持預設關。".into()
            }
            other => format!("沒跑完（{other}），不能宣布贏。保持預設關。"),
        },
    }
}

fn returned_items(
    corpus: &Corpus,
    origin: Millis,
    retrieval: Retrieval,
    k: usize,
) -> Result<Vec<ReturnedItem>> {
    let mut raw = Vec::new();
    for answer in retrieval.answers {
        let kind = SourceKind::from_str_kind(&answer.latest.source_kind)
            .with_context(|| format!("unknown fact source kind: {}", answer.latest.source_kind))?;
        raw.push((
            RetrievalChannel::Fact,
            answer.latest.ts - origin,
            None,
            kind,
            vec![answer.latest.raw, answer.latest.normalized],
        ));
    }
    for activity in retrieval.activities {
        let at_ms = activity.core_started_at - origin;
        let end_ms = activity.core_ended_at - origin;
        let mut values = Vec::new();
        if let Some(title) = activity.title.clone() {
            values.push(title);
        }
        if let Some(app) = activity.app.clone() {
            values.push(app);
        }
        if let Some(host) = activity.host.clone() {
            values.push(host);
        }
        ensure!(
            !values.is_empty(),
            "session chapter at {at_ms}ms has no app/title/host to grade"
        );
        raw.push((
            RetrievalChannel::Session,
            at_ms,
            Some(end_ms),
            SourceKind::WindowTitle,
            values,
        ));
    }
    let hit_channel = match retrieval.shape {
        crate::question::Shape::Recent => RetrievalChannel::Recent,
        crate::question::Shape::Range => RetrievalChannel::Range,
        crate::question::Shape::Keywords => RetrievalChannel::Text,
    };
    for hit in retrieval.hits {
        raw.push((
            hit_channel,
            hit.ts - origin,
            None,
            hit.source_kind,
            vec![hit.text],
        ));
    }

    raw.into_iter()
        .take(k)
        .enumerate()
        .map(|(rank, (channel, at_ms, end_ms, source_kind, values))| {
            ensure!(
                (0..=corpus.duration_ms).contains(&at_ms),
                "retrieval result timestamp {at_ms} fell outside corpus"
            );
            if let Some(end) = end_ms {
                ensure!(
                    at_ms <= end && (0..=corpus.duration_ms).contains(&end),
                    "retrieval result range [{at_ms},{end}) fell outside corpus"
                );
            }
            let event_indexes: Vec<_> = corpus
                .events
                .iter()
                .enumerate()
                .filter_map(|(index, event)| {
                    let time_ok = match end_ms {
                        Some(end) => event.at_ms() >= at_ms && event.at_ms() < end,
                        None => event.at_ms() == at_ms,
                    };
                    (time_ok && event_supports_item(event, source_kind, channel, &values))
                        .then_some(index)
                })
                .collect();
            ensure!(
                !event_indexes.is_empty(),
                "retrieval result at {at_ms}ms ({channel:?}, {}) cannot be mapped back to its corpus event",
                source_kind.as_str()
            );
            Ok(ReturnedItem {
                rank: rank + 1,
                channel,
                at_ms,
                end_ms,
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
    let surfaces = evidence_surfaces(event);
    if channel == RetrievalChannel::Session || channel == RetrievalChannel::Hypothesis {
        // 章節是範圍：涵蓋到的 event 只要有任何可檢索文字就算對得回。
        // L2 假設掛在段落起點，對不回去要在呼叫端 `ensure!`。
        return !surfaces.is_empty();
    }
    surfaces
        .into_iter()
        .filter(|(kind, _)| *kind == source_kind)
        .any(|(_, surface)| match channel {
            RetrievalChannel::Fact => extract(&surface).iter().any(|fact| {
                values
                    .iter()
                    .any(|value| value == &fact.raw || value == &fact.normalized)
            }),
            RetrievalChannel::Text | RetrievalChannel::Recent | RetrievalChannel::Range => {
                values.iter().any(|value| value == &surface)
            }
            RetrievalChannel::Session | RetrievalChannel::Hypothesis => {
                unreachable!("session/hypothesis 走上面那條")
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
        assert_eq!(report.configurations.len(), 3);
        assert_eq!(report.configurations[2].name, "facts_session");
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
        assert!(
            json.contains("\"kind\":\"not_on_path\""),
            "不跑腦必須是 not_on_path，不是量到 0：{json}"
        );
        assert!(
            !json.contains("\"model_calls\":0"),
            "舊的恆 0 欄位不該還在：{json}"
        );
    }

    #[test]
    fn metrics_view_drops_every_free_form_string_and_keeps_failed_question_numbers() {
        let (corpus, questions) = fixture();
        let mut report = evaluate(&corpus, &questions, 5, 1).expect("evaluate");
        report.question_set.review = ReviewStatus::Draft;
        report.evaluator_version = "PRIVATE TEXT".into();
        report.corpus.name = "PRIVATE TEXT".into();
        report.corpus.fingerprint = "PRIVATE TEXT".into();
        report.question_set.name = "PRIVATE TEXT".into();
        report.question_set.fingerprint = "PRIVATE TEXT".into();
        report.parameters.ranking = "PRIVATE TEXT".into();
        for configuration in &mut report.configurations {
            configuration.name = "PRIVATE TEXT".into();
            configuration.questions[0].id = "PRIVATE TEXT".into();
            configuration.questions[0].question = "PRIVATE TEXT".into();
            for returned in &mut configuration.questions[0].returned {
                returned.values = vec!["PRIVATE TEXT".into()];
            }
        }
        // 只壞 citation 也是評測失敗，不能因為 answer_correct 是 true 就消失。
        report.configurations[0].questions[0].answer_correct = true;
        report.configurations[0].questions[0].recalled = Some(true);
        report.configurations[0].questions[0].citation_correct = Some(false);

        let contents = serde_json::to_string(&report).expect("report json");
        let view = metrics_view_from_json(&contents).expect("metrics view");
        let json = serde_json::to_string(&view).expect("view json");
        assert!(view.private_draft);
        assert_eq!(
            view.configurations[0].failed_question_numbers,
            [1],
            "question number should locate the miss without copying its free-form id"
        );
        assert!(!json.contains("PRIVATE TEXT"), "{json}");
        assert!(!json.contains("returned"), "{json}");
        assert!(!json.contains("\"question\""), "{json}");
    }

    #[test]
    fn metrics_view_keeps_zero_zero_denominator_and_unmeasured_apart() {
        let (corpus, questions) = fixture();
        let mut report = evaluate(&corpus, &questions, 5, 1).expect("evaluate");
        for configuration in &mut report.configurations {
            for question in &mut configuration.questions {
                question.recalled = None;
                question.citation_correct = None;
            }
            configuration.metrics.recall_at_k = Fraction::new(0, 0);
            configuration.metrics.citation_accuracy = Fraction::new(0, 0);
        }

        let view = metrics_view(&report).expect("metrics view");
        assert!(
            !view.private_draft,
            "Reviewed corpus 和 Reviewed 題庫不能永遠被畫成 private Draft"
        );
        let metrics = &view.configurations[0];
        assert_eq!(metrics.recall_at_k.passed, 0);
        assert_eq!(metrics.recall_at_k.total, 0);
        assert_eq!(metrics.recall_at_k.rate, None, "zero denominator is null");
        assert_eq!(
            metrics.model,
            ModelUsage::NotOnPath,
            "不跑腦不是量到 0 次呼叫"
        );
        assert_eq!(
            metrics.reminder_false_positive_rate, None,
            "an unmeasured metric stays null"
        );
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

    fn fake_eval_cli(dir: &std::path::Path, sentinel: &std::path::Path) -> BrainEval {
        let script = dir.join("fake-eval-brain.py");
        std::fs::write(
            &script,
            r#"
import json, pathlib, re, sys
payload = sys.stdin.buffer.read()
pathlib.Path(sys.argv[1]).write_bytes(b'spawned')
text = payload.decode('utf-8', 'replace')
if b'PASS_A' in payload or b'PASS_B' in payload:
    out = '{"commitments":[]}'
else:
    m = re.search(r'本段 segment_ref：segment:(\d+)', text)
    ref = f'segment:{m.group(1)}' if m else 'segment:0'
    frames = re.findall(r'frame:(\d+)', text)
    evid = f'frame:{frames[0]}' if frames else 'frame:1'
    out = json.dumps({
        'segment_ref': ref,
        'activity': '在看螢幕',
        'entities': [],
        'confidence': 0.5,
        'evidence_refs': [evid],
        'open_questions': [],
        'commitment_candidates': [],
    })
sys.stdout.buffer.write(out.encode('utf-8'))
"#,
        )
        .expect("script");
        BrainEval {
            command: "python3".into(),
            args: vec![
                script.to_string_lossy().into_owned(),
                sentinel.to_string_lossy().into_owned(),
            ],
        }
    }

    #[test]
    fn ab_without_cli_is_incomplete_and_does_not_shrink_the_set() {
        let (corpus, questions) = fixture();
        let report = evaluate_ab(&corpus, &questions, 5, 1, None).expect("ab");
        let ab = report.ab.as_ref().expect("ab block");
        assert_eq!(ab.questions_total, 2);
        assert_eq!(ab.questions_graded, 2);
        assert!(ab.questions_skipped.is_empty(), "題庫一題都不能偷拿掉");
        assert_eq!(report.configurations.len(), 4);
        assert_eq!(report.configurations[3].name, "interpreter_reviewer");
        assert_eq!(report.configurations[3].questions.len(), 2);
        assert!(matches!(
            report.configurations[3].metrics.model,
            ModelUsage::NotOnPath
        ));
        assert!(matches!(
            ab.gate,
            AbGate::Incomplete { ref reason } if reason == "no_command"
        ));
        let text = format_ab_gate(&ab.gate);
        assert!(text.contains("保持預設關"), "{text}");
        assert!(text.contains("沒跑成"), "{text}");
    }

    #[test]
    fn ab_with_fake_cli_measures_cost_and_can_honestly_lose() {
        let dir = std::env::temp_dir().join(format!("sister-eval-ab-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sentinel = dir.join("spawned");
        let _ = std::fs::remove_file(&sentinel);
        let (corpus, questions) = fixture();
        let brain = fake_eval_cli(&dir, &sentinel);
        let report = evaluate_ab(&corpus, &questions, 5, 1, Some(&brain)).expect("ab");
        let ab = report.ab.as_ref().expect("ab block");
        assert_eq!(ab.questions_total, questions.questions.len());
        assert_eq!(ab.questions_graded, questions.questions.len());
        assert!(ab.questions_skipped.is_empty());

        let treatment = report
            .configurations
            .iter()
            .find(|c| c.name == "interpreter_reviewer")
            .expect("treatment");
        match &treatment.metrics.model {
            ModelUsage::Measured {
                calls,
                source,
                interpreter_unit_usd,
                reviewer_unit_usd,
                ..
            } => {
                assert!(
                    *calls > 0 || ab.brain.skip.is_some(),
                    "跑了腦就要麼有呼叫、要麼寫出跳過原因：{:?}",
                    ab.brain
                );
                assert!(source.contains("SPEC.md §13"), "{source}");
                assert!(source.contains("cost-model.md"), "{source}");
                assert!(*interpreter_unit_usd > 0.0);
                assert!(*reviewer_unit_usd > 0.0);
            }
            ModelUsage::NotOnPath => panic!("給了 CLI 還說沒跑腦"),
        }

        let baseline = report
            .configurations
            .iter()
            .find(|c| c.name == "facts_session")
            .expect("baseline");
        assert!(
            matches!(baseline.metrics.model, ModelUsage::NotOnPath),
            "不跑腦那一路仍是 not_on_path"
        );

        // 假 CLI 回的 activity 幫不上這份題庫，所以通常沒贏。沒贏要講出來。
        let gate = format_ab_gate(&ab.gate);
        assert!(
            gate.contains("保持預設關") || matches!(ab.gate, AbGate::Won),
            "{gate}"
        );
        if !matches!(ab.gate, AbGate::Won) {
            assert!(
                gate.contains("沒贏") || gate.contains("沒有承諾") || gate.contains("沒跑"),
                "沒贏的話不能只印好消息：{gate}"
            );
        }
        let fc = format_false_commitment(&ab.false_commitment);
        if ab.false_commitment.total == 0 {
            assert!(fc.contains("沒有承諾") && fc.contains("不是 0%"), "{fc}");
            assert!(ab.false_commitment.rate.is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn measured_zero_and_not_on_path_do_not_print_the_same() {
        let zero = ModelUsage::Measured {
            calls: 0,
            usd_per_day: 0.0,
            interpreter_calls: 0,
            reviewer_calls: 0,
            interpreter_unit_usd: interpreter_unit_usd(),
            reviewer_unit_usd: reviewer_unit_usd(),
            source: MODEL_PRICE_SOURCE.to_string(),
        };
        let ran = format_model_usage(&zero);
        let skipped = format_model_usage(&ModelUsage::NotOnPath);
        assert_ne!(ran, skipped);
        assert!(ran.contains("跑了腦"), "{ran}");
        assert!(ran.contains("0 次呼叫"), "{ran}");
        assert!(skipped.contains("沒跑腦"), "{skipped}");
        assert!(ran.contains("SPEC.md §13"), "{ran}");
    }

    #[test]
    fn false_commitment_with_zero_denominator_is_not_zero_percent() {
        let none = Fraction::new(0, 0);
        assert_eq!(none.rate, None);
        let text = format_false_commitment(&none);
        assert!(text.contains("沒有承諾"), "{text}");
        assert!(text.contains("不是 0%"), "{text}");
        assert!(!text.contains("0.0%"), "{text}");
    }
}
