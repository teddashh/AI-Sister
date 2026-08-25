//! 承諾集與開口判斷集：綁定某份 replay corpus 的人工標註資料。
//!
//! 這不是守門員，也不會讓產品多講一句話。機器只負責把「值得人看一眼」
//! 的時刻提出來，`label` 一律留 `None` 等人標。SPEC §12 的誤提醒／漏提醒
//! 要等 Phase 5 真的有開口路徑才有分母。

use std::collections::BTreeSet;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::eval::{EvidenceRef, corpus_fingerprint};
use crate::facts::{FactKind, extract};
use crate::model::{Millis, SystemKind};
use crate::replay::{Corpus, Event, ReplayFocus, ReviewStatus};

pub const MOMENT_SET_VERSION: u32 = 1;

/// 連續同一個 focus 超過這段時間，就提出來給人看一眼。
///
/// **這是任意選的起點，不是量過的卡住門檻。** SPEC §8.3c 的卡住偵測還要
/// 反覆切換和 error 事實；這裡只做「同處停留」這一條可解釋的機器線索。
pub const LONG_DWELL_MS: Millis = 5 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MomentSet {
    pub format_version: u32,
    pub name: String,
    pub review: ReviewStatus,
    pub corpus_fingerprint: String,
    pub moments: Vec<LabeledMoment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabeledMoment {
    pub id: String,
    pub at_ms: Millis,
    /// 為什麼這一刻被提出來給人看。是機器算的線索，不是 ground truth。
    pub candidate: CandidateReason,
    /// 支撐這個標註的 corpus event index（不是 DB row id）。
    pub evidence: Vec<EvidenceRef>,
    /// `None` = 還沒有人標。絕不用任何 enum 變體或空字串兼差「沒標」。
    pub label: Option<MomentLabel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateReason {
    /// frame OCR 裡抽到 DateTimeMention 事實。
    DateTimeMention,
    /// System event 通過 [`system_kind_is_notification`] 的檢查。
    Notification,
    /// 同一個 focus 停留超過 [`LONG_DWELL_MS`]。
    LongDwell,
    /// 人自己加的，機器沒提。
    HandPicked,
}

impl CandidateReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DateTimeMention => "date_time_mention",
            Self::Notification => "notification",
            Self::LongDwell => "long_dwell",
            Self::HandPicked => "hand_picked",
        }
    }

    /// 給標註畫面看的那一句；必須對得上機器真正檢查過的那條規則。
    pub fn describe(self) -> &'static str {
        match self {
            Self::DateTimeMention => "畫面 OCR 抽出 DateTimeMention",
            Self::Notification => "系統事件通過通知類檢查",
            Self::LongDwell => "同一個 focus 停留超過門檻",
            Self::HandPicked => "人工加入",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "label", rename_all = "snake_case", deny_unknown_fields)]
pub enum MomentLabel {
    /// 承諾集：使用者在這一刻講出一個承諾。
    Commitment {
        /// 人自己寫的、該提醒的內容。
        remind: String,
        /// 有講明時間才填相對毫秒；沒講時間是 `None`，不是 0。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        due_at_ms: Option<Millis>,
    },
    /// 開口判斷集：這一刻她該講話。
    ShouldSpeak {
        category: SpeakCategory,
        why: String,
    },
    /// 開口判斷集：這一刻她不該講話。
    ShouldStayQuiet { why: String },
}

/// SPEC §8.3 第 299–304 行的五類候選來源，一字不改地對應。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakCategory {
    CommitmentDue,          // a
    UnattendedNotification, // b
    Stuck,                  // c
    SessionEnd,             // d
    Leaving,                // e
}

impl SpeakCategory {
    pub const ALL: [SpeakCategory; 5] = [
        Self::CommitmentDue,
        Self::UnattendedNotification,
        Self::Stuck,
        Self::SessionEnd,
        Self::Leaving,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommitmentDue => "commitment_due",
            Self::UnattendedNotification => "unattended_notification",
            Self::Stuck => "stuck",
            Self::SessionEnd => "session_end",
            Self::Leaving => "leaving",
        }
    }

    pub fn names(sep: &str) -> String {
        Self::ALL
            .into_iter()
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join(sep)
    }
}

impl FromStr for SpeakCategory {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let want = s.trim().to_ascii_lowercase().replace('-', "_");
        let found = match want.as_str() {
            "a" | "commitment_due" => Some(Self::CommitmentDue),
            "b" | "unattended_notification" => Some(Self::UnattendedNotification),
            "c" | "stuck" => Some(Self::Stuck),
            "d" | "session_end" => Some(Self::SessionEnd),
            "e" | "leaving" => Some(Self::Leaving),
            _ => None,
        };
        found.ok_or_else(|| {
            format!(
                "沒有這一類該講來源：{s}（可用的是 {}，或 SPEC §8.3 的 a–e）",
                Self::names("、")
            )
        })
    }
}

/// 人明確確認過這份時刻集裡的原話可以分享。
///
/// 欄位不公開：呼叫端只能拿 [`Self::CONFIRMED`] / [`Self::NOT_CONFIRMED`]，
/// 不能把 `replace` 或 clap 其它 `bool` 塞進來。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfirmPrivateTextReviewed(bool);

impl ConfirmPrivateTextReviewed {
    pub const CONFIRMED: Self = Self(true);
    pub const NOT_CONFIRMED: Self = Self(false);

    pub fn is_confirmed(self) -> bool {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MomentSetCounts {
    pub total: usize,
    pub unlabeled: usize,
    pub commitments: usize,
    pub commitments_with_due: usize,
    pub should_speak: usize,
    pub should_stay_quiet: usize,
}

impl MomentSet {
    /// 掃一遍 corpus，把值得人看一眼的時刻提出來；`label` 一律 `None`。
    pub fn draft_from_corpus(name: &str, corpus: &Corpus) -> Result<Self> {
        corpus.validate()?;
        ensure!(!name.trim().is_empty(), "moment set 缺少名稱");

        let mut moments = Vec::new();
        collect_datetime_mentions(corpus, &mut moments);
        collect_notifications(corpus, &mut moments);
        collect_long_dwells(corpus, &mut moments);
        ensure!(
            !moments.is_empty(),
            "這份 corpus 沒有可標註的時刻候選：沒有畫面 OCR 抽出 DateTimeMention、沒有通過通知類檢查的系統事件、也沒有同一 focus 停留超過 {} ms（這個門檻是任意起點，沒有量過）",
            LONG_DWELL_MS
        );

        moments.sort_by_key(|moment| {
            (
                moment.at_ms,
                moment
                    .evidence
                    .first()
                    .map(|reference| reference.event_index)
                    .unwrap_or(usize::MAX),
                reason_order(moment.candidate),
            )
        });
        for (index, moment) in moments.iter_mut().enumerate() {
            moment.id = format!("moment-{:04}", index + 1);
        }

        let set = Self {
            format_version: MOMENT_SET_VERSION,
            name: name.trim().to_string(),
            review: ReviewStatus::Draft,
            corpus_fingerprint: corpus_fingerprint(corpus)?,
            moments,
        };
        set.validate(corpus)?;
        Ok(set)
    }

    pub fn validate(&self, corpus: &Corpus) -> Result<()> {
        ensure!(
            self.format_version == MOMENT_SET_VERSION,
            "不支援 moment set format_version {}（這版只支援 {}）",
            self.format_version,
            MOMENT_SET_VERSION
        );
        ensure!(!self.name.trim().is_empty(), "moment set 缺少名稱");
        ensure!(!self.moments.is_empty(), "moment set 一個時刻都沒有");
        let expected_fingerprint = corpus_fingerprint(corpus)?;
        ensure!(
            self.corpus_fingerprint == expected_fingerprint,
            "moment set 綁定的 corpus fingerprint 不同：檔案是 {}，corpus 是 {}",
            self.corpus_fingerprint,
            expected_fingerprint
        );

        let mut ids = BTreeSet::new();
        let mut previous_at: Option<Millis> = None;
        for (moment_index, moment) in self.moments.iter().enumerate() {
            ensure!(
                !moment.id.trim().is_empty(),
                "moment #{moment_index} 缺少 id"
            );
            ensure!(
                ids.insert(moment.id.as_str()),
                "moment id 重複：{}",
                moment.id
            );
            ensure!(
                (0..=corpus.duration_ms).contains(&moment.at_ms),
                "moment {} 的 at_ms={} 不在 0..={} 內",
                moment.id,
                moment.at_ms,
                corpus.duration_ms
            );
            if let Some(before) = previous_at {
                ensure!(
                    moment.at_ms >= before,
                    "moment {} 的 at_ms={} 早於前一個時刻 {}",
                    moment.id,
                    moment.at_ms,
                    before
                );
            }
            previous_at = Some(moment.at_ms);

            for reference in &moment.evidence {
                ensure!(
                    reference.event_index < corpus.events.len(),
                    "moment {} 指到不存在的 corpus event #{}",
                    moment.id,
                    reference.event_index
                );
            }

            if let Some(label) = &moment.label {
                ensure!(
                    !moment.evidence.is_empty(),
                    "moment {} 已標註，但沒有 evidence",
                    moment.id
                );
                match label {
                    MomentLabel::Commitment { remind, due_at_ms } => {
                        ensure!(
                            !remind.trim().is_empty(),
                            "moment {} 的承諾 remind 是空的",
                            moment.id
                        );
                        if let Some(due) = *due_at_ms {
                            ensure!(
                                due >= moment.at_ms,
                                "moment {} 的 due_at_ms={due} 早於講出承諾的 at_ms={}",
                                moment.id,
                                moment.at_ms
                            );
                        }
                    }
                    MomentLabel::ShouldSpeak { why, .. } | MomentLabel::ShouldStayQuiet { why } => {
                        ensure!(!why.trim().is_empty(), "moment {} 的 why 是空的", moment.id);
                    }
                }
            } else if self.review == ReviewStatus::Reviewed {
                anyhow::bail!("Reviewed moment set 裡仍有未標註時刻：{}", moment.id);
            }
        }
        Ok(())
    }

    /// 回傳一份只改指定時刻的 Draft；原本的時刻集永遠不會被部分修改。
    pub fn with_label(
        &self,
        corpus: &Corpus,
        id: &str,
        label: MomentLabel,
        replace: bool,
    ) -> Result<Self> {
        self.validate(corpus)?;
        ensure!(
            self.review == ReviewStatus::Draft,
            "Reviewed moment set 不可直接改標註；請從審查前的 Draft 產生新版"
        );

        let mut next = self.clone();
        let moment = next
            .moments
            .iter_mut()
            .find(|moment| moment.id == id)
            .with_context(|| format!("moment id 不存在：{id}"))?;
        ensure!(
            replace || moment.label.is_none(),
            "moment {id} 已經有標註；要更正請明確選擇重新標註"
        );
        moment.label = Some(label);
        next.validate(corpus)?;
        Ok(next)
    }

    /// 每一個 moment 都有 `label` 才准產生另一份 Reviewed。
    pub fn reviewed(&self, corpus: &Corpus) -> Result<Self> {
        self.validate(corpus)?;
        ensure!(
            self.review == ReviewStatus::Draft,
            "moment set 已經是 Reviewed"
        );
        let missing: Vec<_> = self
            .moments
            .iter()
            .filter(|moment| moment.label.is_none())
            .map(|moment| moment.id.as_str())
            .collect();
        ensure!(
            missing.is_empty(),
            "還有 {} 個時刻沒有標註：{}",
            missing.len(),
            missing.join("、")
        );

        let mut reviewed = self.clone();
        reviewed.review = ReviewStatus::Reviewed;
        reviewed.validate(corpus)?;
        Ok(reviewed)
    }

    pub fn counts(&self) -> MomentSetCounts {
        let mut counts = MomentSetCounts {
            total: self.moments.len(),
            ..MomentSetCounts::default()
        };
        for moment in &self.moments {
            match &moment.label {
                None => counts.unlabeled += 1,
                Some(MomentLabel::Commitment { due_at_ms, .. }) => {
                    counts.commitments += 1;
                    if due_at_ms.is_some() {
                        counts.commitments_with_due += 1;
                    }
                }
                Some(MomentLabel::ShouldSpeak { .. }) => counts.should_speak += 1,
                Some(MomentLabel::ShouldStayQuiet { .. }) => counts.should_stay_quiet += 1,
            }
        }
        counts
    }
}

fn reason_order(reason: CandidateReason) -> u8 {
    match reason {
        CandidateReason::DateTimeMention => 0,
        CandidateReason::Notification => 1,
        CandidateReason::LongDwell => 2,
        CandidateReason::HandPicked => 3,
    }
}

fn collect_datetime_mentions(corpus: &Corpus, moments: &mut Vec<LabeledMoment>) {
    for (event_index, event) in corpus.events.iter().enumerate() {
        let Event::Frame { at_ms, ocr, .. } = event else {
            continue;
        };
        let found = ocr.iter().any(|block| {
            extract(&block.text)
                .iter()
                .any(|fact| fact.kind == FactKind::DateTimeMention)
        });
        if !found {
            continue;
        }
        moments.push(unlabeled(
            *at_ms,
            CandidateReason::DateTimeMention,
            vec![event_index],
        ));
    }
}

fn collect_notifications(corpus: &Corpus, moments: &mut Vec<LabeledMoment>) {
    for (event_index, event) in corpus.events.iter().enumerate() {
        let Event::System { at_ms, kind, .. } = event else {
            continue;
        };
        if !system_kind_is_notification(*kind) {
            continue;
        }
        moments.push(unlabeled(
            *at_ms,
            CandidateReason::Notification,
            vec![event_index],
        ));
    }
}

/// 目前的 [`SystemKind`] 沒有通知橫幅這一格。鎖屏、睡眠、暫停、排除、
/// session 邊界都不是通知。
///
/// 每一臂都回 `false` 是現況，不是漏寫。不可改成 `_ => false`：新的
/// `SystemKind` 必須走到這裡決定它是不是通知，不能因為它是 System 就當通知。
#[allow(clippy::match_same_arms)]
fn system_kind_is_notification(kind: SystemKind) -> bool {
    match kind {
        SystemKind::Lock => false,
        SystemKind::Unlock => false,
        SystemKind::Sleep => false,
        SystemKind::Wake => false,
        SystemKind::CapturePaused => false,
        SystemKind::CaptureResumed => false,
        SystemKind::Excluded => false,
        SystemKind::SessionStart => false,
        SystemKind::SessionEnd => false,
    }
}

fn collect_long_dwells(corpus: &Corpus, moments: &mut Vec<LabeledMoment>) {
    let mut open: Option<OpenDwell> = None;
    for (event_index, event) in corpus.events.iter().enumerate() {
        let Some(key) = event_focus_key(event) else {
            continue;
        };
        match open.as_mut() {
            Some(dwell) if dwell.key == key => {
                dwell.evidence.push((event_index, event.at_ms()));
            }
            _ => {
                flush_long_dwell(open.take(), moments);
                open = Some(OpenDwell {
                    key,
                    start_ms: event.at_ms(),
                    evidence: vec![(event_index, event.at_ms())],
                });
            }
        }
    }
    flush_long_dwell(open, moments);
}

struct OpenDwell {
    key: String,
    start_ms: Millis,
    evidence: Vec<(usize, Millis)>,
}

fn flush_long_dwell(dwell: Option<OpenDwell>, moments: &mut Vec<LabeledMoment>) {
    let Some(dwell) = dwell else {
        return;
    };
    let Some((_, last_ms)) = dwell.evidence.last().copied() else {
        return;
    };
    if last_ms.saturating_sub(dwell.start_ms) < LONG_DWELL_MS {
        return;
    }
    let Some((_, at_ms)) = dwell
        .evidence
        .iter()
        .copied()
        .find(|(_, at)| at.saturating_sub(dwell.start_ms) >= LONG_DWELL_MS)
    else {
        return;
    };
    moments.push(unlabeled(
        at_ms,
        CandidateReason::LongDwell,
        dwell
            .evidence
            .into_iter()
            .map(|(event_index, _)| event_index)
            .collect(),
    ));
}

fn event_focus_key(event: &Event) -> Option<String> {
    match event {
        Event::Frame { focus, .. } => focus_key(focus),
        Event::Focus { snapshot, .. } => focus_key(snapshot),
        _ => None,
    }
}

fn focus_key(focus: &ReplayFocus) -> Option<String> {
    let app = focus
        .app_id
        .as_deref()
        .or(focus.app_name.as_deref())
        .map(str::trim)
        .filter(|app| !app.is_empty())?;
    let title = focus
        .window_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("");
    Some(format!("{app}\u{1f}{title}"))
}

fn unlabeled(at_ms: Millis, candidate: CandidateReason, evidence: Vec<usize>) -> LabeledMoment {
    LabeledMoment {
        id: String::new(),
        at_ms,
        candidate,
        evidence: evidence
            .into_iter()
            .map(|event_index| EvidenceRef { event_index })
            .collect(),
        label: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OcrBlock;
    use crate::replay::{FORMAT_VERSION, RedactionSummary};

    fn ocr(text: &str) -> Vec<OcrBlock> {
        vec![OcrBlock {
            text: text.into(),
            x: 0,
            y: 0,
            w: 300,
            h: 20,
            confidence: 1.0,
        }]
    }

    fn frame(at_ms: Millis, app: &str, title: &str, text: &str) -> Event {
        Event::Frame {
            at_ms,
            monitor: 0,
            width: 800,
            height: 600,
            dhash: at_ms as u64,
            dup_run: 0,
            focus: ReplayFocus {
                app_id: Some(app.into()),
                window_title: Some(title.into()),
                ..Default::default()
            },
            ocr: ocr(text),
        }
    }

    fn corpus(duration_ms: Millis, events: Vec<Event>) -> Corpus {
        Corpus {
            format_version: FORMAT_VERSION,
            name: "moment fixture".into(),
            duration_ms,
            review: ReviewStatus::Reviewed,
            redactions: RedactionSummary::default(),
            events,
        }
    }

    fn datetime_corpus() -> Corpus {
        corpus(
            2_000,
            vec![
                frame(200, "chat.exe", "LINE", "下午5點接她"),
                frame(1_500, "editor.exe", "notes", "普通工作筆記"),
            ],
        )
    }

    fn count_reason(set: &MomentSet, reason: CandidateReason) -> usize {
        set.moments
            .iter()
            .filter(|moment| moment.candidate == reason)
            .count()
    }

    #[test]
    fn draft_labels_are_all_none_and_follow_extracted_datetimes() {
        let one = datetime_corpus();
        let first = MomentSet::draft_from_corpus("one", &one).expect("draft");
        assert_eq!(first.review, ReviewStatus::Draft);
        assert!(
            first.moments.iter().all(|moment| moment.label.is_none()),
            "draft 不可猜 ground truth"
        );
        let datetime_one = count_reason(&first, CandidateReason::DateTimeMention);
        assert!(
            datetime_one >= 1,
            "fixture OCR「下午5點接她」必須抽出 DateTimeMention，實際候選：{:?}",
            first
                .moments
                .iter()
                .map(|moment| moment.candidate)
                .collect::<Vec<_>>()
        );

        let mut two = one.clone();
        two.events
            .push(frame(2_000, "mail.exe", "inbox", "明天 17:00 開會"));
        two.duration_ms = 2_000;
        let second = MomentSet::draft_from_corpus("two", &two).expect("draft");
        let datetime_two = count_reason(&second, CandidateReason::DateTimeMention);
        assert_eq!(
            datetime_two,
            datetime_one + 1,
            "多一幀含 DateTimeMention 的畫面，候選數要跟著變，不是寫死"
        );
        assert_eq!(count_reason(&second, CandidateReason::Notification), 0);
    }

    #[test]
    fn money_and_lock_are_not_datetime_or_notification_candidates() {
        let source = corpus(
            3_000,
            vec![
                frame(100, "browser.exe", "帳單", "本期應繳 NT$1,350"),
                Event::System {
                    at_ms: 500,
                    kind: SystemKind::Lock,
                    detail: Some("LINE 通知：會計師來訊".into()),
                },
                frame(2_500, "browser.exe", "帳單", "還是同一張帳單"),
            ],
        );
        let error = MomentSet::draft_from_corpus("none", &source)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("沒有可標註的時刻候選"),
            "金額不是 DateTimeMention，Lock 就算 detail 寫著通知也不是通知類：{error}"
        );
        assert!(
            !system_kind_is_notification(SystemKind::Lock)
                && !system_kind_is_notification(SystemKind::Excluded)
                && !system_kind_is_notification(SystemKind::SessionEnd),
            "現行 SystemKind 沒有通知橫幅；不可因為它是 System 就當通知"
        );
    }

    #[test]
    fn long_dwell_appears_only_after_the_threshold_is_actually_spanned() {
        let short = corpus(
            10_000,
            vec![
                frame(0, "editor.exe", "main.rs", "正在寫"),
                frame(9_000, "editor.exe", "main.rs", "還在寫"),
            ],
        );
        let error = MomentSet::draft_from_corpus("short", &short)
            .unwrap_err()
            .to_string();
        assert!(error.contains("沒有可標註的時刻候選"), "{error}");

        let mut long = short.clone();
        long.events.push(frame(
            LONG_DWELL_MS,
            "editor.exe",
            "main.rs",
            "還在同一個檔",
        ));
        long.duration_ms = LONG_DWELL_MS;
        let set = MomentSet::draft_from_corpus("long", &long).expect("draft");
        assert_eq!(count_reason(&set, CandidateReason::LongDwell), 1);
        assert!(set.moments.iter().all(|moment| moment.label.is_none()));
        let dwell = set
            .moments
            .iter()
            .find(|moment| moment.candidate == CandidateReason::LongDwell)
            .expect("dwell");
        assert!(
            dwell.at_ms >= LONG_DWELL_MS,
            "跨過門檻的那一刻，不是任意寫死的 3：{}",
            dwell.at_ms
        );
    }

    #[test]
    fn fingerprint_mismatch_is_rejected() {
        let corpus = datetime_corpus();
        let mut set = MomentSet::draft_from_corpus("fp", &corpus).expect("draft");
        set.corpus_fingerprint = "fnv1a64:deadbeefdeadbeef".into();
        let error = set.validate(&corpus).unwrap_err().to_string();
        assert!(error.contains("fingerprint"), "{error}");
    }

    #[test]
    fn at_ms_outside_corpus_is_rejected() {
        let corpus = datetime_corpus();
        let mut set = MomentSet::draft_from_corpus("range", &corpus).expect("draft");
        set.moments[0].at_ms = corpus.duration_ms + 1;
        let error = set.validate(&corpus).unwrap_err().to_string();
        assert!(error.contains("at_ms"), "{error}");
    }

    #[test]
    fn due_before_the_commitment_is_rejected() {
        let corpus = datetime_corpus();
        let draft = MomentSet::draft_from_corpus("due", &corpus).expect("draft");
        let id = &draft.moments[0].id;
        let at = draft.moments[0].at_ms;
        let error = draft
            .with_label(
                &corpus,
                id,
                MomentLabel::Commitment {
                    remind: "接她".into(),
                    due_at_ms: Some(at - 1),
                },
                false,
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("due_at_ms"), "{error}");
        assert!(
            draft.moments.iter().all(|moment| moment.label.is_none()),
            "失敗的標註不能寫回來源"
        );
    }

    #[test]
    fn review_refuses_unlabeled_moments_and_says_how_many() {
        let corpus = datetime_corpus();
        let draft = MomentSet::draft_from_corpus("review", &corpus).expect("draft");
        let unlabeled = draft.counts().unlabeled;
        assert!(unlabeled >= 1);
        let error = draft.reviewed(&corpus).unwrap_err().to_string();
        assert!(
            error.contains(&unlabeled.to_string()) && error.contains("沒有標註"),
            "{error}"
        );
    }

    #[test]
    fn with_label_without_replace_refuses_a_second_label() {
        let corpus = datetime_corpus();
        let draft = MomentSet::draft_from_corpus("once", &corpus).expect("draft");
        let id = draft.moments[0].id.clone();
        let one = draft
            .with_label(
                &corpus,
                &id,
                MomentLabel::ShouldStayQuiet {
                    why: "正在寫程式".into(),
                },
                false,
            )
            .expect("first label");
        let error = one
            .with_label(
                &corpus,
                &id,
                MomentLabel::ShouldSpeak {
                    category: SpeakCategory::Stuck,
                    why: "卡住了".into(),
                },
                false,
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("已經有標註"), "{error}");
        let replaced = one
            .with_label(
                &corpus,
                &id,
                MomentLabel::ShouldSpeak {
                    category: SpeakCategory::Stuck,
                    why: "卡住了".into(),
                },
                true,
            )
            .expect("explicit replace");
        assert!(matches!(
            replaced.moments[0].label,
            Some(MomentLabel::ShouldSpeak {
                category: SpeakCategory::Stuck,
                ..
            })
        ));
    }

    #[test]
    fn counts_keep_unlabeled_apart_from_zero_should_speak() {
        let corpus = datetime_corpus();
        let draft = MomentSet::draft_from_corpus("counts", &corpus).expect("draft");
        let unlabeled = draft.counts();
        assert_eq!(unlabeled.unlabeled, unlabeled.total);
        assert_eq!(unlabeled.should_speak, 0);
        assert_eq!(unlabeled.should_stay_quiet, 0);
        assert_eq!(unlabeled.commitments, 0);

        let mut quiet = draft.clone();
        for moment in &draft.moments {
            quiet = quiet
                .with_label(
                    &corpus,
                    &moment.id,
                    MomentLabel::ShouldStayQuiet {
                        why: "現在不該開口".into(),
                    },
                    false,
                )
                .expect("label quiet");
        }
        let labeled = quiet.counts();
        assert_eq!(labeled.unlabeled, 0, "已經標完，未標必須是 0 不是缺測");
        assert_eq!(
            labeled.should_speak, 0,
            "標成 0 個該講，和還沒標的 0 個該講要靠 unlabeled 分開"
        );
        assert_eq!(labeled.should_stay_quiet, labeled.total);
        assert_ne!(unlabeled.unlabeled, labeled.unlabeled);

        let reviewed = quiet.reviewed(&corpus).expect("all labeled");
        assert_eq!(reviewed.review, ReviewStatus::Reviewed);
        assert_eq!(reviewed.counts().unlabeled, 0);
        assert_eq!(reviewed.counts().should_speak, 0);
    }

    #[test]
    fn commitment_due_none_is_not_serialized_as_zero() {
        let label = MomentLabel::Commitment {
            remind: "買牛奶".into(),
            due_at_ms: None,
        };
        let json = serde_json::to_string(&label).expect("json");
        assert!(
            !json.contains("due_at_ms"),
            "沒講時間應省略欄位，不可寫 0：{json}"
        );
        assert!(json.contains("\"label\":\"commitment\""), "{json}");
    }

    #[test]
    fn speak_category_parses_spec_letters_and_rejects_typos() {
        assert_eq!(
            SpeakCategory::from_str("a").expect("a"),
            SpeakCategory::CommitmentDue
        );
        assert_eq!(
            SpeakCategory::from_str("unattended-notification").expect("hyphen"),
            SpeakCategory::UnattendedNotification
        );
        let error = SpeakCategory::from_str("shout").expect_err("typo");
        assert!(error.contains("stuck") && error.contains("a–e"), "{error}");
    }
}
