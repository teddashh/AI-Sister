//! Reviewer：L3 的唯一寫入者。
//!
//! SPEC §6：活躍時 15–30 分一輪 + 日終一輪。不是每秒輪詢。
//! 五類強制回查（金額、人物、承諾、未完成狀態、長期記憶候選）寫進 L3 之前
//! **一定要讀 L0 原件**，不是把 L2 卡片再讀一次。
//!
//! 高風險寫入（新承諾）走雙 pass：平行獨立，禁止互讀。分歧 = 警報，
//! 降 confidence、不寫入 L3。多數決不能消除幻覺。
//!
//! 合併是欄位級 typed card merge，不是把兩張卡片重寫成一段敘事。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::brain::{
    self, CommitmentCandidate, Entity, EvidenceRef, OutboundOutcome, ProcessStart, SpawnOutcome,
    spawn_cli,
};
use crate::config::BrainConfig;
use crate::consent::Consent;
use crate::db::{
    CommitmentInsert, DaySummaryInsert, Db, DivergenceInsert, DualPassDivergences, EntityMemory,
    L2Author, L2CardRow, L2Insert, MISSING_SEGMENT_NOTE_HEAD, NOTHING_TO_REVIEW_HEAD,
    OutboundInsert, RecheckInsert, RecheckStats, ReviewerNotes, ReviewerRunInsert,
};
use crate::model::Millis;

/// 活躍時最短間隔。低於這個再叫就靜默跳過，不是報錯。
pub const MIN_INTERVAL_MS: Millis = 15 * 60_000;
/// 到期後多久沒互動才轉 archived（SPEC §7.2 第 3 條）。
pub const ARCHIVE_GRACE_MS: Millis = 48 * 3_600_000;

/// 只有這個模組鑄得出來的 L3 寫入憑證。
///
/// 不是 `bool`，也不是 `struct L3Write { allowed: bool }`：那種還是能填錯。
/// 元組結構體、欄位私有，同一模組之外沒有別的路。
/// [`crate::db::Db::insert_commitment`] 等寫入函式的第一個參數就是這個型別。
#[derive(Debug, Clone, Copy)]
pub struct L3Write(());

fn l3_write() -> L3Write {
    L3Write(())
}

/// 測試專用。`#[cfg(test)]` 是刻意的：出貨的程式碼裡，L3 的寫入憑證仍然只有
/// 這個模組鑄得出來，別的模組連測試都要靠這一支才拿得到。
#[cfg(test)]
pub(crate) fn test_l3_write() -> L3Write {
    l3_write()
}

/// 一輪在 `run_at` 跑的日終盤點，盤的是哪一天。
///
/// **一份定義，兩個呼叫端。** 日摘要是這裡寫的，而守門員的 d 類候選要拿
/// `daysummary:` 去指同一列——兩邊各自算一次的話，其中一邊改成
/// `local_day_key` 就會變成「昨天的筆記做好了」配一個今天的 id，
/// 或是反過來每天都問一次一份早就寫好的筆記。兩句話都很自然，
/// 而且沒有任何一行是假的。
pub fn summarized_day(run_at: Millis) -> Option<String> {
    // 日終盤點跑完多半已經過午夜，所以盤的是前一天，不是「今天」。
    brain::previous_local_day_key(run_at)
}

/// 為什麼這一輪沒審。每一種印出來的字都不一樣。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    NoConsent,
    NoCommand,
    BudgetExhausted { used: u32, limit: u32 },
    Cadence { last_ago_ms: Millis, min_ms: Millis },
    NothingToReview { remaining: u32 },
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::NoConsent => "no_consent",
            SkipReason::NoCommand => "no_command",
            SkipReason::BudgetExhausted { .. } => "budget",
            SkipReason::Cadence { .. } => "cadence",
            SkipReason::NothingToReview { .. } => "nothing",
        }
    }

    pub fn message(&self) -> String {
        self.message_with_consent_command("sister consent --grant cloud-reading")
    }

    pub fn message_with_consent_command(&self, consent_command: &str) -> String {
        match self {
            SkipReason::NoConsent => format!(
                "還沒簽第二張同意書（上雲解讀）。審閱層一次都不會呼叫那支 CLI。\n要簽字：{consent_command}"
            ),
            SkipReason::NoCommand => concat!(
                "還沒設定 [brain] command。審閱層一次都不會呼叫。\n",
                "（不是今天沒有東西可審——她根本沒有一支 CLI 可以叫。）"
            )
            .to_string(),
            SkipReason::BudgetExhausted { used, limit } => {
                format!("今天的審閱預算已用完（{used}/{limit}）。超過即靜默降級，不寫 L3。")
            }
            SkipReason::Cadence {
                last_ago_ms,
                min_ms,
            } => {
                let mins = last_ago_ms / 60_000;
                let need = min_ms / 60_000;
                format!(
                    "上一輪審閱才過 {mins} 分鐘（最短間隔 {need} 分鐘）。不是每秒輪詢，這一輪跳過。"
                )
            }
            SkipReason::NothingToReview { remaining } => format!(
                "{NOTHING_TO_REVIEW_HEAD}。（同意書已簽、CLI 已設定、預算還剩 {remaining} 次。）"
            ),
        }
    }
}

/// 回查率那一行。兩種 0 不准長得一樣。
pub fn format_recheck_rate(s: &RecheckStats) -> String {
    match s.runs {
        None => match &s.last_skip {
            Some(reason) => format!(
                "審閱層一次都還沒跑成（最近一次停在：{}）。回查率還沒量到。",
                skip_reason_label(reason)
            ),
            None => "審閱層一次都還沒跑過，回查率還沒量到。".into(),
        },
        Some(n) => match (s.candidates, s.rechecks) {
            (Some(0), Some(0)) => {
                format!("審閱層跑過 {n} 輪，沒有五類寫入需要回查原件。")
            }
            (Some(c), Some(0)) => {
                format!("審閱層跑過 {n} 輪，有 {c} 筆五類候選，一次都沒回查原件。")
            }
            (Some(c), Some(r)) if c > 0 => {
                let pct = (r as f64) * 100.0 / (c as f64);
                format!("審閱層跑過 {n} 輪，回查 {r}/{c}（{pct:.0}%）。")
            }
            (Some(c), Some(r)) => {
                format!("審閱層跑過 {n} 輪，回查 {r} 次（候選 {c}）。")
            }
            _ => format!("審閱層跑過 {n} 輪，回查次數這份資料沒記全。"),
        },
    }
}

pub fn format_review_result(r: &ReviewResult, stats: &RecheckStats) -> String {
    format_review_result_with_consent_command(r, stats, "sister consent --grant cloud-reading")
}

pub fn format_review_result_with_consent_command(
    r: &ReviewResult,
    stats: &RecheckStats,
    consent_command: &str,
) -> String {
    let mut out = String::new();
    if let Some(skip) = &r.skip {
        if r.ran {
            out.push_str(&skip.message_with_consent_command(consent_command));
            out.push('\n');
        } else {
            out.push_str("真的跑的話會停在這裡：\n");
            out.push_str(&skip.message_with_consent_command(consent_command));
            out.push('\n');
        }
    } else {
        out.push_str(&format!(
            "審閱結束：回查 {}／{}，寫入 {} 筆承諾，分歧 {} 筆，L2 修訂 {} 張。\n",
            r.rechecks, r.candidates, r.wrote_commitments, r.divergences, r.l2_revisions
        ));
        // 0 的時候不印。印「拒絕 0 個下一步」會讓每一輪都長得像出過事。
        if r.refused_next_steps > 0 {
            out.push_str(&format!(
                "她拒絕了 {} 個模型指的下一步（理由見底下）。\n",
                r.refused_next_steps
            ));
        }
        if r.dropped_evidence_refs > 0 {
            out.push_str(&format!(
                "她丟掉了 {} 筆模型引用、但這次根本沒給模型看過的證據；這不代表承諾被拒絕。\n",
                r.dropped_evidence_refs
            ));
        }
        // 0 的時候不印，理由和上面那句一樣。而且**不能併進「拒絕了 N 個下一步」**：
        // 這幾張卡片模型連看都沒看到，那句話講的是「模型指了、她不接受」。
        if r.cards_missing_segment > 0 {
            out.push_str(&format!(
                "有 {} 張卡片指的那一段現在查不到，這一輪沒有給它們任何畫面上的 fact（理由見底下）。\n",
                r.cards_missing_segment
            ));
        }
        if r.calls_used > 0 {
            out.push_str(&format!(
                "今日審閱呼叫 {}/{}（這一輪用了 {} 次）。\n",
                r.budget_used, r.budget_limit, r.calls_used
            ));
        } else {
            out.push_str(&format!(
                "這一輪沒呼叫模型（預算還剩 {}/{}）。\n",
                r.budget_limit.saturating_sub(r.budget_used),
                r.budget_limit
            ));
        }
        if r.completed > 0 {
            out.push_str(&format!("後續證據把 {} 筆標成完成。\n", r.completed));
        }
        if r.archived > 0 {
            out.push_str(&format!("到期未互動、轉進封存 {} 筆。\n", r.archived));
        }
    }
    out.push_str(&format_recheck_rate(stats));
    out.push('\n');
    out
}

/// 每一輪審閱都會印一次實體記憶，而實體與提及是**只增不減**的。
/// 用了幾週之後整份印出來就是一面牆，所以有上限；上限本身不是問題，
/// 沒講出來才是（見底下兩處「沒列出來」）。
const MAX_ENTITIES_SHOWN: usize = 20;
const MAX_MENTIONS_SHOWN: usize = 6;

pub fn format_reviewer_visibility(
    divergences: &DualPassDivergences,
    refusals: &crate::db::ReviewerRefusals,
    notes: &ReviewerNotes,
    entities: &EntityMemory,
) -> String {
    // 分歧是**警報**，不是統計行的續行。前後各空一行，讓它自己是一段。
    let mut out = String::from("\n");
    match divergences {
        DualPassDivergences::NeverRan => {
            out.push_str("雙 pass 還沒跑過；目前沒有可比較的兩份答案。\n");
        }
        DualPassDivergences::NoComparableAnswers { run_id, rows } => {
            out.push_str(&format!(
                "最近一次雙 pass（審閱輪次 #{run_id}）沒有拿到可比較的兩份答案：\n"
            ));
            for row in rows {
                out.push_str(&format!(
                    "- {}\n  原因：{}\n  pass A：{}\n  pass B：{}\n",
                    row.subject, row.reason, row.pass_a_json, row.pass_b_json
                ));
            }
        }
        DualPassDivergences::Agreed { run_id } => {
            out.push_str(&format!(
                "最近一次雙 pass（審閱輪次 #{run_id}）沒有分歧。\n"
            ));
        }
        DualPassDivergences::Diverged { run_id, rows } => {
            out.push_str(&format!("最近一次雙 pass（審閱輪次 #{run_id}）的分歧：\n"));
            for row in rows {
                out.push_str(&format!(
                    "- {}\n  原因：{}\n  pass A：{}\n  pass B：{}\n",
                    row.subject, row.reason, row.pass_a_json, row.pass_b_json
                ));
            }
        }
    }
    out.push('\n');
    // 拒絕不放進上面那一段。那一段的意思是「兩份答案對不上」，而拒絕的時候
    // 兩份答案是一致的——是她不接受它們指的那筆 fact。
    //
    // 前兩塊取的是同一列（`skip_reason IS NULL AND calls_used > 0` 的最新
    // 一列）：分歧回答「兩份答案對不對得上」，拒絕回答「她拒了哪幾步」。
    // 標籤不同是因為它們回答那一列的不同問題，不是因為取了不同的列。
    // `answers_got = 0` 的那一輪兩塊一起改口：不是分歧、也沒資格說她拒了
    // 什麼。拒絕那一臂的標籤是「試著問模型」，和「問過模型」、notes 的
    // 「最近一次審閱」兩兩不同。第三塊（說明）讀最新一列
    // `skip_reason IS NULL`（含零次呼叫），才可能是更新的一列。
    match refusals {
        crate::db::ReviewerRefusals::NeverRan => {
            out.push_str("審閱還沒問過模型；目前沒有資格回答她拒絕了什麼。\n\n");
        }
        crate::db::ReviewerRefusals::GotNoAnswer { run_id } => {
            out.push_str(&format!(
                "最近一次試著問模型的審閱（輪次 #{run_id}）沒拿到模型的答案；目前沒有資格回答她拒絕了什麼。\n\n"
            ));
        }
        crate::db::ReviewerRefusals::None { run_id } => {
            out.push_str(&format!(
                "最近一次問過模型的審閱（輪次 #{run_id}）沒有拒絕任何下一步。\n\n"
            ));
        }
        crate::db::ReviewerRefusals::Some { run_id, reasons } => {
            out.push_str(&format!(
                "最近一次問過模型的審閱（輪次 #{run_id}）拒絕掉的下一步：\n"
            ));
            for reason in reasons {
                out.push_str(&format!("- {reason}\n"));
            }
            out.push('\n');
        }
    }
    match notes {
        ReviewerNotes::NeverRan | ReviewerNotes::None { .. } => {}
        ReviewerNotes::Some { run_id, lines } => {
            out.push_str(&format!("最近一次審閱（輪次 #{run_id}）另外記下的說明：\n"));
            for line in lines {
                out.push_str(&format!("- {line}\n"));
            }
            out.push('\n');
        }
    }
    match entities {
        EntityMemory::NeverReviewed => {
            out.push_str("實體記憶還沒跑過 Reviewer；目前不是『辨識到 0 個』。\n");
        }
        EntityMemory::GotNoAnswer => {
            out.push_str("Reviewer 試過問模型，但沒拿到答案；目前不是『辨識到 0 個』。\n");
        }
        EntityMemory::Empty => out.push_str("Reviewer 跑過，但目前沒有活著的實體。\n"),
        EntityMemory::Present(rows) => {
            if rows.len() > MAX_ENTITIES_SHOWN {
                out.push_str(&format!(
                    "目前辨識到的實體（共 {} 個，以下只列 {MAX_ENTITIES_SHOWN} 個）：\n",
                    rows.len()
                ));
            } else {
                out.push_str(&format!("目前辨識到的實體（共 {} 個）：\n", rows.len()));
            }
            for row in rows.iter().take(MAX_ENTITIES_SHOWN) {
                // 「段」是這一行自己的宣稱，所以要先去重再數。`entity_mentions`
                // 上沒有 `(entity_id, seen_ref)` 的 UNIQUE，同一張卡每輪插兩列
                // （承諾裡的人 + 卡上的實體），而 `latest_unreviewed` 不排除審過
                // 的卡——數列的話，一段會被說成一百多段。
                let mut seen = BTreeSet::new();
                let segments: Vec<&str> = row
                    .mentions
                    .iter()
                    .map(|m| m.seen_ref.as_str())
                    .filter(|r| seen.insert(*r))
                    .collect();
                let shown = segments
                    .iter()
                    .take(MAX_MENTIONS_SHOWN)
                    .copied()
                    .collect::<Vec<_>>();
                let refs = if shown.is_empty() {
                    "（目前沒有活著的提及）".into()
                } else if segments.len() > MAX_MENTIONS_SHOWN {
                    // 剪掉尾巴的清單跟完整的清單長得一模一樣，所以每一次剪都要
                    // 自己講出來。少講這一句，「出現於：六段」就變成一句假話。
                    format!(
                        "{}（另有 {} 段沒列出來）",
                        shown.join("、"),
                        segments.len() - MAX_MENTIONS_SHOWN
                    )
                } else {
                    shown.join("、")
                };
                out.push_str(&format!(
                    "- [{}] {}；出現於：{}\n",
                    row.entity.kind, row.entity.name, refs
                ));
            }
            if rows.len() > MAX_ENTITIES_SHOWN {
                out.push_str(&format!(
                    "還有 {} 個實體沒列出來；完整的在資料庫的 entities 表。\n",
                    rows.len() - MAX_ENTITIES_SHOWN
                ));
            }
        }
    }
    out
}

fn skip_reason_label(reason: &str) -> &'static str {
    match reason {
        "no_consent" => "沒簽同意書 2",
        "no_command" => "沒設定 [brain] command",
        "budget" => "預算用完",
        "cadence" => "還沒到最短間隔",
        "nothing" => "沒有可審的假設",
        _ => "不明原因",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecheckCategory {
    Money,
    Person,
    Commitment,
    OpenState,
    LongTerm,
}

impl RecheckCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Money => "money",
            Self::Person => "person",
            Self::Commitment => "commitment",
            Self::OpenState => "open_state",
            Self::LongTerm => "long_term",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    Interval,
    Eod,
}

impl ReviewKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interval => "interval",
            Self::Eod => "eod",
        }
    }
}

pub struct ReviewInput<'a> {
    pub db: &'a mut Db,
    pub consent: &'a Consent,
    pub brain: &'a BrainConfig,
    pub from_ts: Millis,
    pub to_ts: Millis,
    pub kind: ReviewKind,
    /// 不理 15 分鐘間隔。預算、同意書仍然算。
    pub force: bool,
    pub now: Millis,
}

#[derive(Debug, Clone)]
pub struct ReviewResult {
    pub skip: Option<SkipReason>,
    pub ran: bool,
    pub rechecks: u32,
    pub candidates: u32,
    pub wrote_commitments: u32,
    pub divergences: u32,
    /// 模型指了一步、而她不接受的次數。**和 `divergences` 是兩個問題。**
    /// 那個數的是「兩份答案對不上」，這個數的是「兩份答案一致，但我拒絕了」。
    pub refused_next_steps: u32,
    /// 模型引用了、而 prompt 根本沒給它看的 ref，被丟掉幾筆。
    pub dropped_evidence_refs: u32,
    /// 卡片指的那一段現在查不到，於是這一輪不給它任何 L1 fact。
    ///
    /// **這不是 `refused_next_steps`。** 那個數的是「模型指了一步、而她不接受」，
    /// 這個數的是「模型連看都還沒看，她就先把這張卡片的 fact 清單清空了」——
    /// 發生在呼叫模型**之前**，而且卡片可能因為 `five_class` 算出來是空的就直接
    /// 跳過，模型從頭到尾沒參與。混進去會讓「她拒絕了 N 個**模型指的**下一步」
    /// 變成假話：實測過一次模型只指了一步、那個數字卻是 2。
    pub cards_missing_segment: u32,
    pub calls_used: u32,
    pub budget_used: u32,
    pub budget_limit: u32,
    pub l2_revisions: u32,
    pub archived: u32,
    pub completed: u32,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ReviewPassCard {
    pub commitments: Vec<ReviewCommitment>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ReviewCommitment {
    pub text: String,
    #[serde(default)]
    pub stands: bool,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub due_hint: Option<String>,
    #[serde(default)]
    pub due_source: Option<String>,
    #[serde(default)]
    pub people: Vec<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    /// 兩個 pass 都列到的 ref。模型回覆沒有這一欄——`skip` 讓它塞進來也不會被讀。
    #[serde(skip)]
    pub agreed_evidence_refs: Vec<String>,
    #[serde(default)]
    pub allowed_next_step: Option<NextStepRef>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NextStepRef {
    pub fact: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub subject: String,
    pub reason: String,
    pub pass_a_json: String,
    pub pass_b_json: String,
}

/// 欄位級合併。對不上就整筆分歧，不投票。
pub fn merge_commitment_passes(
    a: &ReviewCommitment,
    b: &ReviewCommitment,
) -> Result<ReviewCommitment, Divergence> {
    let subject = normalize_text(&a.text);
    if subject != normalize_text(&b.text) {
        return Err(diverge("text", a, b));
    }
    if a.stands != b.stands {
        return Err(diverge("stands", a, b));
    }
    if a.kind.as_deref().map(normalize_text) != b.kind.as_deref().map(normalize_text) {
        return Err(diverge("kind", a, b));
    }
    if a.due_hint.as_deref().map(normalize_text) != b.due_hint.as_deref().map(normalize_text) {
        return Err(diverge("due_hint", a, b));
    }
    if a.due_source.as_deref().map(normalize_text) != b.due_source.as_deref().map(normalize_text) {
        return Err(diverge("due_source", a, b));
    }
    let pa: BTreeSet<_> = a.people.iter().map(|p| normalize_text(p)).collect();
    let pb: BTreeSet<_> = b.people.iter().map(|p| normalize_text(p)).collect();
    if pa != pb {
        return Err(diverge("people", a, b));
    }
    let mut evidence: BTreeSet<String> = BTreeSet::new();
    evidence.extend(a.evidence_refs.iter().cloned());
    evidence.extend(b.evidence_refs.iter().cloned());
    // 聯集是「她這一輪看了什麼」；交集才是「什麼可以替一步背書」。
    // 不讀 a/b 的 `agreed_evidence_refs`：那一欄是合併算的，模型塞進來的值
    // 在反序列化時已被 skip 丟掉，這裡再算一次，兩邊對得起來。
    let a_refs: BTreeSet<String> = a.evidence_refs.iter().cloned().collect();
    let b_refs: BTreeSet<String> = b.evidence_refs.iter().cloned().collect();
    let agreed: Vec<String> = a_refs.intersection(&b_refs).cloned().collect();
    let conf = match (a.confidence, b.confidence) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    };
    Ok(ReviewCommitment {
        text: a.text.clone(),
        stands: a.stands,
        kind: a.kind.clone(),
        due_hint: a.due_hint.clone(),
        due_source: a.due_source.clone(),
        people: a.people.clone(),
        confidence: conf,
        evidence_refs: evidence.into_iter().collect(),
        agreed_evidence_refs: agreed,
        allowed_next_step: if a.allowed_next_step == b.allowed_next_step {
            a.allowed_next_step.clone()
        } else {
            None
        },
    })
}

fn diverge(field: &str, a: &ReviewCommitment, b: &ReviewCommitment) -> Divergence {
    Divergence {
        subject: a.text.clone(),
        reason: format!("欄位 {field} 對不上"),
        pass_a_json: serde_json::to_string(a).unwrap_or_default(),
        pass_b_json: serde_json::to_string(b).unwrap_or_default(),
    }
}

fn normalize_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn merge_l2_fields(
    base: &L2CardRow,
    keep_entities: &[Entity],
    keep_commitments: &[CommitmentCandidate],
) -> (String, String) {
    let entities_json = serde_json::to_string(keep_entities).unwrap_or_else(|_| "[]".into());
    let commitments_json = serde_json::to_string(keep_commitments).unwrap_or_else(|_| "[]".into());
    let _ = base;
    (entities_json, commitments_json)
}

pub fn five_class(card: &L2CardRow, facts: &[crate::db::FactRow]) -> Vec<RecheckCategory> {
    let mut out = Vec::new();
    let entities: Vec<Entity> = serde_json::from_str(&card.entities_json).unwrap_or_default();
    let commits: Vec<CommitmentCandidate> =
        serde_json::from_str(&card.commitments_json).unwrap_or_default();
    let questions: Vec<String> =
        serde_json::from_str(&card.open_questions_json).unwrap_or_default();

    if facts.iter().any(|f| f.kind == "money")
        || card.activity.contains("NT$")
        || card.activity.contains('$')
    {
        out.push(RecheckCategory::Money);
    }
    if entities.iter().any(|e| e.kind == "person") {
        out.push(RecheckCategory::Person);
    }
    if !commits.is_empty() {
        out.push(RecheckCategory::Commitment);
    }
    if !questions.is_empty() {
        out.push(RecheckCategory::OpenState);
    }
    if entities
        .iter()
        .any(|e| e.kind == "project" || e.kind == "org")
    {
        out.push(RecheckCategory::LongTerm);
    }
    out
}

pub fn due_source_from_originals(
    due_hint: &str,
    originals: &[crate::db::L0Original],
) -> &'static str {
    let needle = due_hint.trim();
    if needle.is_empty() {
        return "inferred";
    }
    let hit = originals
        .iter()
        .any(|o| o.text.contains(needle) || clock_mentioned(&o.text, needle));
    if hit { "explicit" } else { "inferred" }
}

fn clock_mentioned(text: &str, hint: &str) -> bool {
    // 「17:00」和「五點」對得上才算螢幕上寫了。
    if hint.contains("17")
        && (text.contains('五') && text.contains('點')
            || text.contains("5:00")
            || text.contains("17:00"))
    {
        return true;
    }
    text.contains(hint)
}

pub fn parse_due_at(hint: &str, evidence_ts: Millis) -> Option<Millis> {
    let hint = hint.trim();
    let parts: Vec<&str> = hint.split(':').collect();
    if parts.len() == 2 {
        let h: i64 = parts[0].parse().ok()?;
        let m: i64 = parts[1].parse().ok()?;
        if !(0..24).contains(&h) || !(0..60).contains(&m) {
            return None;
        }
        let local =
            chrono::DateTime::from_timestamp_millis(evidence_ts)?.with_timezone(&chrono::Local);
        let due = local.date_naive().and_hms_opt(h as u32, m as u32, 0)?;
        let due = due.and_local_timezone(chrono::Local).single()?;
        return Some(due.timestamp_millis());
    }
    None
}

pub fn run(input: &mut ReviewInput<'_>) -> Result<ReviewResult> {
    // `run_day` 是「這一輪是哪一天跑的」（reviewer_run.day_key / 每日預算）。
    // 日摘要要盤點的是昨天，見下面 Eod 分支的 `summarized_day`。兩個不能共用。
    let run_day = brain::local_day_key(input.now).context("算不出今天的日期，不敢審")?;
    let used = input
        .db
        .brain_outbound_count_on_role(&run_day, "reviewer")?;
    let limit = input.brain.reviewer_daily_budget;
    let remaining = limit.saturating_sub(used);

    if input.consent.cloud_permit().is_none() {
        record_skip(input, SkipReason::NoConsent, &run_day, used, limit)?;
        return Ok(skipped(SkipReason::NoConsent, used, limit));
    }
    if input.brain.cli().is_none() {
        record_skip(input, SkipReason::NoCommand, &run_day, used, limit)?;
        return Ok(skipped(SkipReason::NoCommand, used, limit));
    }
    if remaining == 0 {
        let reason = SkipReason::BudgetExhausted { used, limit };
        record_skip(input, reason.clone(), &run_day, used, limit)?;
        return Ok(skipped(reason, used, limit));
    }
    if !input.force
        && input.kind == ReviewKind::Interval
        && let Some(last) = input.db.last_reviewer_run_at()?
    {
        let ago = input.now.saturating_sub(last);
        if ago < MIN_INTERVAL_MS {
            let reason = SkipReason::Cadence {
                last_ago_ms: ago,
                min_ms: MIN_INTERVAL_MS,
            };
            record_skip(input, reason.clone(), &run_day, used, limit)?;
            return Ok(skipped(reason, used, limit));
        }
    }
    if input.kind == ReviewKind::Eod
        && let Some(prev) = input.db.last_reviewer_eod_day()?
        && prev == run_day
        && !input.force
    {
        let reason = SkipReason::Cadence {
            last_ago_ms: 0,
            min_ms: MIN_INTERVAL_MS,
        };
        record_skip(input, reason.clone(), &run_day, used, limit)?;
        return Ok(skipped(reason, used, limit));
    }

    let cards = latest_unreviewed(input.db, input.from_ts, input.to_ts)?;
    if cards.is_empty() && input.kind == ReviewKind::Interval {
        let reason = SkipReason::NothingToReview { remaining };
        // 這是「看過了、沒有東西」——記成一次真正跑過、候選 0。
        input.db.insert_reviewer_run(&ReviewerRunInsert {
            ts: input.now,
            day_key: &run_day,
            kind: input.kind.as_str(),
            skip_reason: None,
            candidate_count: Some(0),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 0,
            budget_used: used as i64,
            budget_limit: limit as i64,
            detail: "",
            notes: &reason.message(),
            answers_got: None,
        })?;
        return Ok(ReviewResult {
            skip: Some(reason),
            ran: true,
            rechecks: 0,
            candidates: 0,
            wrote_commitments: 0,
            divergences: 0,
            refused_next_steps: 0,
            dropped_evidence_refs: 0,
            cards_missing_segment: 0,
            calls_used: 0,
            budget_used: used,
            budget_limit: limit,
            l2_revisions: 0,
            archived: 0,
            completed: 0,
            detail: String::new(),
        });
    }

    let permit = input.consent.cloud_permit().expect("上面已經擋過沒簽的");
    let (command, args) = input
        .brain
        .cli()
        .map(|(c, a)| (c.to_string(), a.to_vec()))
        .expect("上面已經擋過沒命令的");

    let mut candidates = 0u32;
    let mut rechecks = 0u32;
    let mut wrote = 0u32;
    let mut divergences = 0u32;
    // 她拒絕掉的下一步。和 `divergences` 分開數：一個是「兩份答案對不上」，
    // 一個是「兩份答案一致，而我不接受它們指的那筆 fact」。
    let mut refusals: Vec<String> = Vec::new();
    // 這不是「拒絕下一步」；分開保存，避免污染 reviewer_run.detail 和拒絕計數。
    let mut dropped_evidence_refs: Vec<String> = Vec::new();
    // 這也不是「拒絕下一步」，理由同上一行，但踩到的坑更深一點：它發生在**呼叫
    // 模型之前**，所以連「模型指了一步」這個前提都還不成立。混進 `refusals` 的話
    // ——我實測過——模型只指了一步，畫面上會印「她拒絕了 2 個模型指的下一步」。
    let mut missing_segments: Vec<String> = Vec::new();
    let mut calls = 0u32;
    // 幾個 pass 真的問到了（提示送完、時限內結束、stdout 能 parse）。
    // 和 `calls` 分開：那個數的是試了幾次（也是扣了幾次額度）。
    // spawn 失敗或逾時，stdout 裡就算有 JSON 也不算。
    let mut answers_got = 0u32;
    let mut l2_revisions = 0u32;
    let mut budget_left = remaining;

    let mut recheck_rows: Vec<RecheckInsertOwned> = Vec::new();
    let mut divergence_rows: Vec<(String, String, String, String)> = Vec::new();

    for card in &cards {
        if card.author == L2Author::User {
            continue;
        }
        // 這張卡片自己那一段，就是它能看到的全部。**不加 `OVERLAP_MARGIN_MS`**：
        // margin 住在 `started_at`/`ended_at` 那一對（`segment.rs:383`），core 那一對
        // 是精確相接的（`segment.rs` 的 `核心邊界該相接` 那條測試），配上
        // `facts_in_range` 的半開區間，邊界上的 fact 歸後面那一段，不重不漏。
        // 加了 margin 等於把授權視窗伸進**下一段** 5 秒——那正是這次要修的 bug，
        // 只是縮小版。而且 `brain.rs` 和字母人抓 fact 都用不含 margin 的那一對，
        // 寬過它就會出現「當初寫這張卡時模型沒看過、現在卻能當授權目標」的 fact。
        let facts = match input.db.segment_core_end(card.segment_core_start)? {
            Some(core_ended_at) => input
                .db
                .facts_in_range(card.segment_core_start, core_ended_at)?,
            None => {
                // 查不到那一段：fail-closed。退回一小時是 fail-open。
                // **兩種成因都一筆 fact 都不給**——原始事件還在、甚至有一段
                // 蓋住，也不准把視窗放到現在那一段去。授權視窗是這張卡自己
                // 的 core。
                //
                // 記在 `missing_segments` 不是 `refusals`：這裡還沒呼叫模型，
                // 而且底下 `cats.is_empty()` 可能讓這張卡直接跳過，模型從頭到尾
                // 沒參與。句子也不能借 `TargetApp::Forgotten` 那一句——那句講的是
                // 「**下一步的目標**那筆 fact 的來源沒了」，這裡沒了的是
                // 「**這張卡片自己那一段**」，兩件事。
                //
                // 講哪句話問的是原始資料，不是 segment 快取。covering 只當
                // 附加資訊：有蓋住就提一句，沒蓋住不准因此改口說被忘掉。
                missing_segments.push(missing_segment_line(
                    card,
                    RawRecordsInCoreWindow::probe(input.db, card.segment_core_start)?,
                    input.db.covering_segment_at(card.segment_core_start)?,
                ));
                Vec::new()
            }
        };
        let cats = five_class(card, &facts);
        if cats.is_empty() && input.kind != ReviewKind::Eod {
            continue;
        }
        let refs: Vec<String> = serde_json::from_str(&card.evidence_json).unwrap_or_default();
        let evidence: Vec<EvidenceRef> =
            refs.iter().filter_map(|s| EvidenceRef::parse(s)).collect();
        let mut originals = Vec::new();
        for r in &evidence {
            candidates += 1;
            let orig = input.db.l0_original(r)?;
            let present = orig.is_some();
            let chars = orig.as_ref().map(|o| o.text.chars().count() as i64);
            let matched = orig.as_ref().is_some_and(|o| !o.text.trim().is_empty());
            rechecks += 1;
            recheck_rows.push(RecheckInsertOwned {
                category: cats
                    .first()
                    .map(|c| c.as_str())
                    .unwrap_or("long_term")
                    .to_string(),
                child_ref: format!("l2:{}", card.id),
                parent_ref: r.as_str(),
                original_present: present,
                original_chars: chars,
                matched,
            });
            if let Some(o) = orig {
                originals.push(o);
            }
        }
        let shown_refs = refs_shown_to_model(&originals, &facts);
        // 五類各自記一筆「我有去讀原件」，即使共用同一組 evidence。
        for cat in cats.iter().skip(1) {
            if let Some(r) = evidence.first() {
                candidates += 1;
                rechecks += 1;
                let orig = originals.iter().find(|o| o.r#ref == r.as_str());
                recheck_rows.push(RecheckInsertOwned {
                    category: cat.as_str().to_string(),
                    child_ref: format!("l2:{}", card.id),
                    parent_ref: r.as_str(),
                    original_present: orig.is_some(),
                    original_chars: orig.map(|o| o.text.chars().count() as i64),
                    matched: orig.is_some_and(|o| !o.text.trim().is_empty()),
                });
            }
        }

        if originals.is_empty() {
            continue;
        }

        let original_blob: String = originals
            .iter()
            .map(|o| o.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let entities: Vec<Entity> = serde_json::from_str(&card.entities_json).unwrap_or_default();
        let keep_entities: Vec<Entity> = entities
            .into_iter()
            .filter(|e| original_blob.contains(&e.name))
            .collect();
        let commits: Vec<CommitmentCandidate> =
            serde_json::from_str(&card.commitments_json).unwrap_or_default();
        let keep_commits: Vec<CommitmentCandidate> = commits
            .into_iter()
            .filter(|c| original_blob.contains(&c.text) || original_blob.contains(&c.source))
            .collect();

        let (ent_json, com_json) = merge_l2_fields(card, &keep_entities, &keep_commits);
        let changed = ent_json != card.entities_json || com_json != card.commitments_json;
        if changed {
            input.db.insert_l2_card(&L2Insert {
                segment_core_start: card.segment_core_start,
                segment_ref: &card.segment_ref,
                activity: &card.activity,
                entities_json: ent_json,
                continues_json: card.continues_json.clone(),
                commitments_json: com_json,
                model_confidence: card.model_confidence,
                evidence_json: card.evidence_json.clone(),
                open_questions_json: card.open_questions_json.clone(),
                author: L2Author::Reviewer,
            })?;
            l2_revisions += 1;
        }

        if keep_commits.is_empty() {
            continue;
        }
        if budget_left < 2 {
            // 雙 pass 要兩次。不夠就這筆不寫，靜默降級。
            continue;
        }

        let prompt_a = dual_pass_prompt('A', card, &originals, &facts, &keep_commits)?;
        let prompt_b = dual_pass_prompt('B', card, &originals, &facts, &keep_commits)?;
        debug_assert!(
            prompt_a.contains("PASS_A")
                && prompt_b.contains("PASS_B")
                && !prompt_a.contains("PASS_B")
                && !prompt_b.contains("PASS_A"),
            "雙 pass 的 prompt 不准提到另一個 pass"
        );

        let (spawn_a, spawn_b) = {
            let cmd = command.as_str();
            let args = args.as_slice();
            std::thread::scope(|scope| {
                let ha = scope.spawn(|| spawn_cli(permit, &prompt_a, cmd, args));
                let hb = scope.spawn(|| spawn_cli(permit, &prompt_b, cmd, args));
                (
                    ha.join()
                        .unwrap_or_else(|_| empty_spawn("pass A 執行緒炸了")),
                    hb.join()
                        .unwrap_or_else(|_| empty_spawn("pass B 執行緒炸了")),
                )
            })
        };
        calls += 2;
        budget_left = budget_left.saturating_sub(2);
        log_outbound(input.db, &run_day, &command, &args, card, &spawn_a)?;
        log_outbound(input.db, &run_day, &command, &args, card, &spawn_b)?;

        // `answers_got` 和後面寫承諾的 match 用同一套判準：提示送完、沒有逾時、
        // CLI 正常退出，stdout 也能 parse，才算問到。
        let parsed_a = parse_usable_pass(&spawn_a);
        let parsed_b = parse_usable_pass(&spawn_b);
        if parsed_a.is_some() {
            answers_got += 1;
        }
        if parsed_b.is_some() {
            answers_got += 1;
        }
        match (parsed_a, parsed_b) {
            (Some(a), Some(b)) => {
                let map_a = by_text(&a.commitments);
                let map_b = by_text(&b.commitments);
                let keys: BTreeSet<_> = map_a.keys().chain(map_b.keys()).cloned().collect();
                for key in keys {
                    match (map_a.get(&key), map_b.get(&key)) {
                        (Some(ca), Some(cb)) => match merge_commitment_passes(ca, cb) {
                            Ok(mut merged) if merged.stands => {
                                merged.evidence_refs.retain(|reference| {
                                    if shown_refs.contains(reference) {
                                        true
                                    } else {
                                        dropped_evidence_refs.push(reference.clone());
                                        false
                                    }
                                });
                                // 交集過同一道濾網。濾掉的不算進 `dropped_evidence_refs`：
                                // 那句話數的是聯集裡丟掉的筆數，同一筆 ref 數兩次會讓它變假話。
                                merged
                                    .agreed_evidence_refs
                                    .retain(|reference| shown_refs.contains(reference));
                                if ca.allowed_next_step != cb.allowed_next_step {
                                    divergences += 1;
                                    divergence_rows.push((
                                        format!("l2:{} / {}", card.id, merged.text),
                                        serde_json::to_string(ca).unwrap_or_default(),
                                        serde_json::to_string(cb).unwrap_or_default(),
                                        "欄位 allowed_next_step 對不上；承諾保留，下一步拿掉"
                                            .into(),
                                    ));
                                }
                                // 拒絕**不進** `reviewer_divergence`。那張表的意思是
                                // 「兩份答案對不上」，畫面上印的是「pass A：… pass B：…」。
                                // 兩個 pass 明明講了同一句話而我拒絕了它，記成分歧的話，
                                // 讀的人會看到一句「分歧」配上兩段一模一樣的 JSON。
                                let allowed_next_step = resolve_allowed_next_step(
                                    input.db,
                                    &facts,
                                    merged.allowed_next_step.as_ref(),
                                )?;
                                let (allowed_next_step, allowed_next_step_fact) =
                                    match allowed_next_step {
                                        ResolvedNextStep::NotAsked => (None, None),
                                        ResolvedNextStep::Resolved { json, fact_id } => {
                                            (Some(json), Some(fact_id))
                                        }
                                        ResolvedNextStep::Refused(reason) => {
                                            refusals.push(reason);
                                            (None, None)
                                        }
                                    };
                                let source = due_source_from_originals(
                                    merged.due_hint.as_deref().unwrap_or(""),
                                    &originals,
                                );
                                let due_at = merged
                                    .due_hint
                                    .as_deref()
                                    .and_then(|h| parse_due_at(h, card.segment_core_start));
                                let people_json = serde_json::to_string(&merged.people)?;
                                let evidence_json = serde_json::to_string(&merged.evidence_refs)?;
                                let agreed_evidence_json =
                                    serde_json::to_string(&merged.agreed_evidence_refs)?;
                                let kind = merged.kind.as_deref().unwrap_or("followup");
                                input.db.insert_commitment(
                                    l3_write(),
                                    &CommitmentInsert {
                                        text: &merged.text,
                                        kind,
                                        born_from: card.id,
                                        evidence_json,
                                        agreed_evidence_json: Some(agreed_evidence_json),
                                        people_json,
                                        due_hint: merged.due_hint.as_deref(),
                                        due_source: Some(source),
                                        due_at,
                                        status: "open",
                                        confidence: merged
                                            .confidence
                                            .unwrap_or(card.model_confidence)
                                            .min(card.model_confidence),
                                        allowed_next_step: allowed_next_step.as_deref(),
                                        allowed_next_step_fact,
                                        last_evidence_seen_at: Some(input.now),
                                        kill_note: None,
                                        now: input.now,
                                    },
                                )?;
                                wrote += 1;
                                for p in &merged.people {
                                    let eid = input.db.upsert_entity(
                                        l3_write(),
                                        "person",
                                        p,
                                        &format!("l2:{}", card.id),
                                        input.now,
                                    )?;
                                    input.db.insert_entity_mention(
                                        l3_write(),
                                        eid,
                                        &format!("l2:{}", card.id),
                                        input.now,
                                    )?;
                                }
                            }
                            Ok(_) => {}
                            Err(d) => {
                                divergences += 1;
                                divergence_rows.push((
                                    format!("l2:{} / {}", card.id, d.subject),
                                    d.pass_a_json,
                                    d.pass_b_json,
                                    d.reason,
                                ));
                            }
                        },
                        _ => {
                            divergences += 1;
                            let a_json =
                                serde_json::to_string(&map_a.get(&key)).unwrap_or_default();
                            let b_json =
                                serde_json::to_string(&map_b.get(&key)).unwrap_or_default();
                            divergence_rows.push((
                                format!("l2:{} / {key}", card.id),
                                a_json,
                                b_json,
                                "只有其中一個 pass 看到這筆承諾".into(),
                            ));
                        }
                    }
                }
            }
            pair => {
                if pair.0.is_some() || pair.1.is_some() {
                    divergences += 1;
                }
                divergence_rows.push((
                    format!("l2:{} / {}", card.id, card.activity),
                    pass_excerpt(&spawn_a),
                    pass_excerpt(&spawn_b),
                    no_usable_json_reason(pair, &spawn_a, &spawn_b),
                ));
            }
        }

        for e in keep_entities {
            if e.kind == "person" || e.kind == "project" || e.kind == "org" || e.kind == "app" {
                let eid = input.db.upsert_entity(
                    l3_write(),
                    &e.kind,
                    &e.name,
                    &format!("l2:{}", card.id),
                    input.now,
                )?;
                input.db.insert_entity_mention(
                    l3_write(),
                    eid,
                    &format!("l2:{}", card.id),
                    input.now,
                )?;
            }
        }
    }

    let mut completed = 0u32;
    let mut archived = 0u32;
    if input.kind == ReviewKind::Eod {
        let summarized_day =
            summarized_day(input.now).context("算不出被盤點的那一天，不敢寫日摘要")?;
        completed = mark_done_from_originals(input)?;
        archived = archive_overdue(input.db, input.now)?;
        write_day_summary(input, &summarized_day)?;
    }

    let run_id_val = input.db.insert_reviewer_run(&ReviewerRunInsert {
        ts: input.now,
        day_key: &run_day,
        kind: input.kind.as_str(),
        skip_reason: None,
        candidate_count: Some(candidates as i64),
        recheck_count: Some(rechecks as i64),
        wrote_commitments: wrote as i64,
        divergences: divergences as i64,
        calls_used: calls as i64,
        budget_used: (used + calls) as i64,
        budget_limit: limit as i64,
        // detail 只裝真的拒絕。卡片那一段不見了是另一塊，見 `notes`。
        detail: &refusals.join("\n"),
        notes: &missing_segments.join("\n"),
        answers_got: Some(answers_got as i64),
    })?;
    for row in recheck_rows {
        input.db.insert_reviewer_recheck(&RecheckInsert {
            run_id: run_id_val,
            category: &row.category,
            child_ref: &row.child_ref,
            parent_ref: &row.parent_ref,
            original_present: row.original_present,
            original_chars: row.original_chars,
            matched: row.matched,
        })?;
    }
    for (subject, a, b, reason) in &divergence_rows {
        input.db.insert_reviewer_divergence(&DivergenceInsert {
            run_id: run_id_val,
            subject,
            pass_a_json: a,
            pass_b_json: b,
            reason,
            created_at: input.now,
        })?;
    }

    Ok(ReviewResult {
        skip: None,
        ran: true,
        rechecks,
        candidates,
        wrote_commitments: wrote,
        divergences,
        refused_next_steps: refusals.len() as u32,
        dropped_evidence_refs: dropped_evidence_refs.len() as u32,
        cards_missing_segment: missing_segments.len() as u32,
        calls_used: calls,
        budget_used: used + calls,
        budget_limit: limit,
        l2_revisions,
        archived,
        completed,
        detail: String::new(),
    })
}

struct RecheckInsertOwned {
    category: String,
    child_ref: String,
    parent_ref: String,
    original_present: bool,
    original_chars: Option<i64>,
    matched: bool,
}

/// 這張卡的 core 窗裡還有沒有任何一種原始紀錄。newtype：不能拿
/// `covering_segment_at(..).is_some()` 塞進同一個參數槽。
struct RawRecordsInCoreWindow(bool);

impl RawRecordsInCoreWindow {
    fn probe(db: &Db, core_started_at: Millis) -> Result<Self> {
        Ok(Self(db.has_raw_records_in_core_window(core_started_at)?))
    }
}

/// 「切法變了」只准出現在這裡。主句不准平述這個成因。
/// 括號掛在「現在查不到」上：這是查不到的成因舉例，不是「紀錄還在」的舉例。
const CUT_CHANGED_AS_EXAMPLE: &str =
    "例如章節的切法變了——你在時間軸上合併過章節，那段範圍被重新計算過";

fn missing_segment_line(
    card: &L2CardRow,
    raw: RawRecordsInCoreWindow,
    covering: Option<(Millis, Millis)>,
) -> String {
    // 走到這裡時手上只有：`segment_core_end(core) == None`，以及窗裡
    // 有沒有原始紀錄。窗長是章節上限 [`crate::segment::TIME_CAP_MS`]，
    // 不是這一段的長度——程式界不出「那一段」，所以句子的主詞是
    // 「這張卡片的起點之後 N 分鐘內」，不是「那一段時間」。
    let cap_min = crate::segment::TIME_CAP_MS / 60_000;
    if raw.0 {
        // covering 是附加資訊，不是分辨法。有蓋住就提；沒蓋住仍只講
        // 看得到的：那一段查不到、起點之後一個上限內還留著紀錄。
        // 「切法變了」是舉例，不是從兩個 probe 推得出來的成因——
        // 快取空的、從來沒人算過，也會長成這個狀態。成因整段只出現在
        // 「現在查不到」後面的括號裡，不在主句。
        //
        // 快取裡確實有一段蓋住那個時間點時，「還沒算過」已被排除，
        // 不准再當例子出現在同一句。
        let covering_clause = match covering {
            Some((start, _)) if start != card.segment_core_start => "，那個時間點仍被另一段蓋著",
            _ => "",
        };
        let uncomputed_example = if covering.is_some() {
            ""
        } else {
            "，或那段範圍還沒算過"
        };
        format!(
            "{MISSING_SEGMENT_NOTE_HEAD}（{}）現在查不到（{CUT_CHANGED_AS_EXAMPLE}{uncomputed_example}）；那張卡片的起點之後{cap_min}分鐘內還留著紀錄{covering_clause}，所以這一輪沒有給模型任何畫面上的 fact，也沒有替它解下一步。",
            card.segment_ref
        )
    } else {
        // 這一臂只講兩個觀察：那一段查不到，起點之後一個上限內也沒有
        // 原始紀錄。不准點名成因。
        //
        // 卡片為什麼通常走不到這裡：forget 刪 segment 用的是重疊
        // （`ended_at > from AND started_at < to`，`retention.rs`），收卡片
        // 卻是包含（`segment_core_start >= from AND < to`，
        // `collect_cascade_parents`）。一段跨過 `from` 的章節，segment 列
        // 被刪掉，卡片的 core 卻可能落在墓碑範圍之外。真正接住它的是血緣：
        // `collect_cascade_parents` 寫 `parent = segment:{...}`，
        // `tombstone_descendants` 走過去。那條血緣有洞——`migrate_012`
        // 不回填，alpha.58 就在用的卡片一列 provenance 都沒有；對那些舊
        // 卡片，落差是開著的。這一輪不修 migrate。
        //
        // prune 不是這條路。它刪 segment 用 `ended_at < text_cut`（單邊
        // 時間點），血緣起點是 `collect_cascade_parents(0, text_cut)`，
        // 沒有「跨過 from」可跨；過了保留期的卡片會被收進 parents、連根
        // 墓碑，走不到這一臂。
        format!(
            "{MISSING_SEGMENT_NOTE_HEAD}（{}）現在查不到，而且那張卡片的起點之後{cap_min}分鐘內也沒有留下任何原始紀錄，所以這一輪沒有給模型任何畫面上的 fact，也沒有替它解下一步。",
            card.segment_ref
        )
    }
}

fn latest_unreviewed(db: &Db, from_ts: Millis, to_ts: Millis) -> Result<Vec<L2CardRow>> {
    let all = db.l2_in_range(from_ts, to_ts)?;
    let mut best: BTreeMap<Millis, L2CardRow> = BTreeMap::new();
    for row in all {
        best.entry(row.segment_core_start)
            .and_modify(|cur| {
                if row.version > cur.version || (row.version == cur.version && row.id > cur.id) {
                    *cur = row.clone();
                }
            })
            .or_insert(row);
    }
    Ok(best.into_values().collect())
}

fn dual_pass_prompt(
    angle: char,
    card: &L2CardRow,
    originals: &[crate::db::L0Original],
    facts: &[crate::db::FactRow],
    commits: &[CommitmentCandidate],
) -> Result<String> {
    let (who, rule, marker) = match angle {
        'A' => (
            "審閱者甲",
            "你的角度：這些承諾候選是不是真的寫在原件上。寧可漏掉，不要承認沒寫到的。",
            "PASS_A",
        ),
        _ => (
            "審閱者乙",
            "你的角度：獨立重讀原件。你看不到任何人的結論。只看原件，不看他人敘事。",
            "PASS_B",
        ),
    };
    let mut s = String::new();
    s.push_str("你是本機記憶的");
    s.push_str(who);
    s.push('（');
    s.push_str(marker);
    s.push_str("）。只輸出一個 JSON 物件，不要 markdown。\n");
    s.push_str(rule);
    s.push_str("\n契約：\n");
    s.push_str(
        r#"{
  "commitments": [
    {"text":"...","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":["..."],"confidence":0.7,"evidence_refs":["frame:1"],"allowed_next_step":{"fact":45}}
  ]
}
"#,
    );
    s.push_str("due_source 只能是 explicit（螢幕上寫了時間）或 inferred（你從上下文猜的）。\n");
    s.push_str("kind 只能是 promise / todo / followup / reminder。\n\n");
    s.push_str("allowed_next_step 只能填下方列出的 fact id；只有 url / file_path 有意義，沒有適合的就填 null。\n\n");
    let mut data = String::new();
    data.push_str("L2 假設（可推翻，不是原件）：\n");
    data.push_str(&format!("- activity: {}\n", card.activity));
    data.push_str("承諾候選：\n");
    for c in commits {
        data.push_str(&format!(
            "- {}（來源 {}） due={:?}\n",
            c.text, c.source, c.due_hint
        ));
    }
    data.push_str("可指向的 L1 facts：\n");
    for fact in facts {
        data.push_str(&format!(
            "- fact:{} kind={} raw={}\n",
            fact.id, fact.kind, fact.raw
        ));
    }
    data.push_str("\n—— L0 原件（真的去讀的，不是 L2 再抄一次）——\n");
    for o in originals {
        data.push_str(&format!("[{}] {}\n", o.r#ref, o.text));
    }
    let (fenced, truncated) = crate::prompt_fence::fence_untrusted_data(&data, usize::MAX)?;
    debug_assert!(!truncated);
    s.push_str(&fenced);
    Ok(s)
}

/// 這一次 prompt 真的把哪些 ref 拿給模型看了。
///
/// **必須和 `dual_pass_prompt` 印出去的那兩行同一個來源**：那支函式印
/// `fact:{id}`（facts）和 `[{r#ref}]`（originals），這裡就只能收這兩樣。
/// 有人往 prompt 裡加第三種識別字而沒改這裡的話，多出來的那種會被當成
/// 捏造的丟掉——方向是 fail-closed，不是放行。
fn refs_shown_to_model(
    originals: &[crate::db::L0Original],
    facts: &[crate::db::FactRow],
) -> std::collections::BTreeSet<String> {
    originals
        .iter()
        .map(|original| original.r#ref.clone())
        .chain(facts.iter().map(|fact| format!("fact:{}", fact.id)))
        .collect()
}

/// 模型指的那一步，解出來是什麼。**三種，不是「一個 json 加一個 rejection」。**
///
/// 兩個 `Option` 併排的話，`(Some, Some)` 這種不可能的狀態在型別上是打得出來
/// 的，而且它會安靜地變成「有下一步，而且我拒絕了它」。分成三格之後打不出來。
enum ResolvedNextStep {
    /// 模型說 `null`。**這不是拒絕**——沒有人要求過任何事。
    NotAsked,
    Resolved {
        json: String,
        fact_id: i64,
    },
    Refused(String),
}

fn resolve_allowed_next_step(
    db: &Db,
    listed_facts: &[crate::db::FactRow],
    next: Option<&NextStepRef>,
) -> Result<ResolvedNextStep> {
    let Some(next) = next else {
        return Ok(ResolvedNextStep::NotAsked);
    };
    let Some(fact) = db.fact_by_id(next.fact)? else {
        return Ok(ResolvedNextStep::Refused(format!(
            "拒絕 allowed_next_step：fact:{} 不存在",
            next.fact
        )));
    };
    if !listed_facts.iter().any(|listed| listed.id == fact.id) {
        return Ok(ResolvedNextStep::Refused(format!(
            "拒絕 allowed_next_step：fact:{} 沒有列在這次給模型的 L1 facts",
            fact.id
        )));
    }
    let listed = listed_facts
        .iter()
        .find(|listed| listed.id == fact.id)
        .expect("前一道檢查已證明這筆 fact 在清單裡");
    if listed.raw != fact.raw {
        return Ok(ResolvedNextStep::Refused(format!(
            "拒絕 allowed_next_step：fact:{} 那一列已經不是當初列給模型看的那一列",
            fact.id
        )));
    }
    let value = match fact.kind.as_str() {
        // 下游 `sister_hands::target_policy::validate_url` 才是真正的權威。
        // 這裡先看一眼，是為了不要端一顆按下去一定會被擋的按鈕給人。
        "url" if fact.raw.starts_with("http://") || fact.raw.starts_with("https://") => {
            serde_json::json!({"action": "open_url", "url": fact.raw})
        }
        "url" => {
            return Ok(ResolvedNextStep::Refused(format!(
                "拒絕 allowed_next_step：fact:{} 的 url 不是以 http:// 或 https:// 開頭",
                fact.id
            )));
        }
        "file_path" => serde_json::json!({"action": "open_file", "path": fact.raw}),
        other => {
            return Ok(ResolvedNextStep::Refused(format!(
                "拒絕 allowed_next_step：fact:{} 的 kind 是 {other}，不是 url 或 file_path",
                fact.id
            )));
        }
    };
    Ok(ResolvedNextStep::Resolved {
        json: serde_json::to_string(&value)?,
        fact_id: fact.id,
    })
}

/// 畫面上 pass A／B 那兩行。`spawn_error` 在的時候 stdout **不一定空**：
/// CLI 立刻退的那條路特地先把管子讀乾淨，真正有用的常常是它印的
/// （沒登入、參數不對），不是我們的「Broken pipe」。兩邊都給人看，各截 400 字。
fn pass_excerpt(spawn: &SpawnOutcome) -> String {
    const CAP: usize = 400;
    let stdout: String = spawn.stdout.chars().take(CAP).collect();
    if let Some(error) = &spawn.spawn_error {
        let error: String = error.chars().take(CAP).collect();
        if stdout.is_empty() {
            error
        } else {
            format!("{error}；CLI 說：{stdout}")
        }
    } else if spawn.timed_out {
        if stdout.is_empty() {
            "逾時".into()
        } else {
            format!("逾時；CLI 說：{stdout}")
        }
    } else {
        stdout
    }
}

/// 一個 pass 為什麼沒有可用的 JSON。只有 [`ProcessStart::NeverStarted`]
/// 才說「叫不起 CLI」；行程起來了的，講它自己的那件事。
const COULD_NOT_START_CLI: &str = "叫不起 CLI";
const PASS_TIMED_OUT: &str = "逾時";
const NO_USABLE_JSON: &str = "沒有可用的 JSON";

fn unusable_pass_clause(spawn: &SpawnOutcome) -> (bool, String) {
    match spawn.process_start {
        ProcessStart::NeverStarted => (true, COULD_NOT_START_CLI.into()),
        ProcessStart::Unobserved => (
            false,
            spawn
                .spawn_error
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "執行緒炸了".into()),
        ),
        ProcessStart::Started if spawn.timed_out => (true, PASS_TIMED_OUT.into()),
        ProcessStart::Started => {
            if let Some(error) = &spawn.spawn_error {
                (true, error.clone())
            } else {
                (true, NO_USABLE_JSON.into())
            }
        }
    }
}

/// 依兩個 pass 的實際狀態組原因。最後一臂是防禦性 fallback；目前呼叫端已先
/// 接走兩份都能 parse 的狀態，不替那條到不了的路宣稱成因。
fn no_usable_json_reason(
    pair: (Option<ReviewPassCard>, Option<ReviewPassCard>),
    spawn_a: &SpawnOutcome,
    spawn_b: &SpawnOutcome,
) -> String {
    let (prefix_a, a) = unusable_pass_clause(spawn_a);
    let (prefix_b, b) = unusable_pass_clause(spawn_b);
    let labelled_a = if prefix_a {
        format!("pass A {a}")
    } else {
        a.clone()
    };
    let labelled_b = if prefix_b {
        format!("pass B {b}")
    } else {
        b.clone()
    };
    match (pair.0.is_some(), pair.1.is_some()) {
        (false, false) => {
            if prefix_a
                && prefix_b
                && a == b
                && (a == COULD_NOT_START_CLI || a == PASS_TIMED_OUT || a == NO_USABLE_JSON)
            {
                format!("兩個 pass 都{a}")
            } else {
                format!("{labelled_a}，{labelled_b}")
            }
        }
        (true, false) => labelled_b,
        (false, true) => labelled_a,
        _ => "其中一個 pass 沒有可用的 JSON".into(),
    }
}

fn parse_usable_pass(spawn: &SpawnOutcome) -> Option<ReviewPassCard> {
    if !spawn.completed_the_ask() {
        return None;
    }
    parse_pass(&spawn.stdout)
}

fn parse_pass(stdout: &str) -> Option<ReviewPassCard> {
    let trimmed = stdout.trim();
    let value = if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        v
    } else {
        let start = trimmed.find('{')?;
        let end = trimmed.rfind('}')?;
        serde_json::from_str(&trimmed[start..=end]).ok()?
    };
    serde_json::from_value(value).ok()
}

fn by_text(items: &[ReviewCommitment]) -> BTreeMap<String, ReviewCommitment> {
    let mut m = BTreeMap::new();
    for c in items {
        m.insert(normalize_text(&c.text), c.clone());
    }
    m
}

fn empty_spawn(msg: &str) -> SpawnOutcome {
    SpawnOutcome {
        payload_chars_written: 0,
        duration_ms: 0,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        spawn_error: Some(msg.into()),
        exit_code: None,
        process_start: ProcessStart::Unobserved,
    }
}

fn unanswered_cli_error(exit_code: Option<i32>) -> String {
    match exit_code {
        Some(code) => format!("CLI 以退出碼 {code} 結束，沒有回答"),
        None => "CLI 被中止了，沒有回答".into(),
    }
}

/// **這裡刻意收不到 payload。**
///
/// `chars_sent` 要記的是「真的離開這台機器的字數」，而那件事只有
/// [`SpawnOutcome::payload_chars_written`] 量得到：`spawn_cli` 在 spawn 失敗時
/// 是在寫 stdin **之前**就 return 的，那一路一個位元組都沒送出去。
///
/// 手上有 payload 的話，`payload.chars().count()` 永遠寫得出來，而且看起來
/// 完全正確——alpha.77 之前這裡就是那樣，於是一台沒裝那支 CLI 的機器在
/// `sister brain log` 上每一輪都寫著送出去幾千個字。**收不到那份資料，這個
/// 錯就寫不出來**；三個寫入點裡這一個是最後被發現的，因為它在另一個模組。
fn log_outbound(
    db: &mut Db,
    day: &str,
    command: &str,
    args: &[String],
    card: &L2CardRow,
    spawn: &SpawnOutcome,
) -> Result<()> {
    let (outcome, error) = if !spawn.completed_the_ask() {
        if spawn.timed_out {
            (OutboundOutcome::Timeout, Some(PASS_TIMED_OUT.into()))
        } else {
            (
                OutboundOutcome::SpawnFailed,
                spawn
                    .spawn_error
                    .clone()
                    .or_else(|| Some(unanswered_cli_error(spawn.exit_code))),
            )
        }
    } else if parse_pass(&spawn.stdout).is_none() {
        (OutboundOutcome::BadJson, Some("JSON 不能用".into()))
    } else {
        (OutboundOutcome::Success, None)
    };
    db.insert_brain_outbound(&OutboundInsert {
        ts: crate::now_ms(),
        day_key: day,
        command,
        args,
        segment_core_start: Some(card.segment_core_start),
        chars_sent: spawn.payload_chars_written as i64,
        truncated: false,
        outcome: outcome.as_str(),
        duration_ms: spawn.duration_ms as i64,
        error: error.as_deref(),
        role: "reviewer",
    })?;
    Ok(())
}

fn record_skip(
    input: &mut ReviewInput<'_>,
    reason: SkipReason,
    day: &str,
    used: u32,
    limit: u32,
) -> Result<()> {
    input.db.insert_reviewer_run(&ReviewerRunInsert {
        ts: input.now,
        day_key: day,
        kind: input.kind.as_str(),
        skip_reason: Some(reason.as_str()),
        candidate_count: None,
        recheck_count: None,
        wrote_commitments: 0,
        divergences: 0,
        calls_used: 0,
        budget_used: used as i64,
        budget_limit: limit as i64,
        detail: &reason.message(),
        notes: "",
        answers_got: None,
    })?;
    // 外送紀錄面板讀 `brain_skip`。審閱層沒送出去的原因要跟解釋層同一張表，
    // 不然「今天沒送」只看得到一半。
    input
        .db
        .insert_brain_skip(input.now, reason.as_str(), None, &reason.message())?;
    Ok(())
}

fn skipped(reason: SkipReason, used: u32, limit: u32) -> ReviewResult {
    ReviewResult {
        skip: Some(reason),
        ran: false,
        rechecks: 0,
        candidates: 0,
        wrote_commitments: 0,
        divergences: 0,
        refused_next_steps: 0,
        dropped_evidence_refs: 0,
        cards_missing_segment: 0,
        calls_used: 0,
        budget_used: used,
        budget_limit: limit,
        l2_revisions: 0,
        archived: 0,
        completed: 0,
        detail: String::new(),
    }
}

fn mark_done_from_originals(input: &mut ReviewInput<'_>) -> Result<u32> {
    let mut n = 0u32;
    let open: Vec<_> = input
        .db
        .live_commitments()?
        .into_iter()
        .filter(|c| c.status == "open")
        .collect();
    for c in open {
        let refs: Vec<String> = serde_json::from_str(&c.evidence_json).unwrap_or_default();
        let mut done = false;
        for r in refs.iter().filter_map(|s| EvidenceRef::parse(s)) {
            if let Some(orig) = input.db.l0_original(&r)?
                && looks_completed(&orig.text)
            {
                done = true;
                break;
            }
        }
        if done {
            input.db.update_commitment_status(
                l3_write(),
                c.id,
                "done",
                Some("後續證據顯示完成畫面"),
                input.now,
            )?;
            n += 1;
        }
    }
    Ok(n)
}

fn looks_completed(text: &str) -> bool {
    text.contains("已完成")
        || text.contains("弄好了")
        || text.contains("done")
        || text.contains("Done")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveDecision {
    NoDue,
    NotOpen { status: String },
    NotDueYet { due_at: Millis },
    GracePeriod { archive_after: Millis },
    InteractionUnknown { archive_after: Millis },
    Archive,
}

/// 現行 schema 沒有「使用者最後何時與這張卡互動」這一格：`updated_at` 是所有
/// L3 狀態寫入共用的時間，`last_evidence_seen_at` 是螢幕證據。
///
/// **但這不表示這一列答不出規則 3。** 那兩件事的不對稱在這裡：
///
/// - `updated_at != created_at` → 有人寫過這一列，而寫的人可能是 Reviewer
///   也可能是使用者。**這一邊真的分不出來**，所以回 `InteractionUnknown`，
///   不歸檔。
/// - `updated_at == created_at` → `insert_commitment`（`db.rs` 的
///   `VALUES(…,?14,?14,NULL)`）把兩格寫成同一個值，而之後**任何一種**寫入都
///   會把 `updated_at` 推走。所以這一格相等的意思是「從誕生到現在一次寫入都
///   沒有」——使用者的互動當然也在「沒有」裡面。這一邊是確定的。
///
/// 規則 3 要的是後面那一種確定性。上一版把整個判斷收成
/// `InteractionUnknown`，於是 `archived` 這個 status **一條路都不剩**，
/// `archive_overdue` 永遠回 0——那不是誠實，那是把規則關掉。
pub fn archive_decision(c: &crate::db::CommitmentRow, now: Millis) -> ArchiveDecision {
    if c.status != "open" {
        return ArchiveDecision::NotOpen {
            status: c.status.clone(),
        };
    }
    let Some(due_at) = c.due_at else {
        return ArchiveDecision::NoDue;
    };
    if now < due_at {
        return ArchiveDecision::NotDueYet { due_at };
    }
    let archive_after = due_at.saturating_add(ARCHIVE_GRACE_MS);
    if now < archive_after {
        return ArchiveDecision::GracePeriod { archive_after };
    }
    if c.updated_at != c.created_at {
        return ArchiveDecision::InteractionUnknown { archive_after };
    }
    ArchiveDecision::Archive
}

/// EOD 定期走到這裡；只有純判斷明確回 Archive 才寫入。
pub fn archive_overdue(db: &mut Db, now: Millis) -> Result<u32> {
    let mut n = 0u32;
    for c in db.live_commitments()? {
        match archive_decision(&c, now) {
            ArchiveDecision::Archive => {
                n += db.update_commitment_status(l3_write(), c.id, "archived", None, now)? as u32;
            }
            ArchiveDecision::NoDue
            | ArchiveDecision::NotOpen { .. }
            | ArchiveDecision::NotDueYet { .. }
            | ArchiveDecision::GracePeriod { .. }
            | ArchiveDecision::InteractionUnknown { .. } => {}
        }
    }
    Ok(n)
}

const FOLLOWUP_PREFERENCE: &str = "followup:last_asked";

pub fn followup_state(db: &Db) -> Result<Option<crate::followup::FollowupState>> {
    let Some(row) = db.preference(FOLLOWUP_PREFERENCE)? else {
        return Ok(None);
    };
    let Some((id, at)) = row.value.split_once(':') else {
        return Ok(None);
    };
    // 解不出卡號就回「沒問過」，**不要編一個 `-1`**。`-1` 會被
    // `followup::decide` 當成一個真的卡號拿去比對，於是「上次問的那張」這件事
    // 被一張不存在的卡佔走，真正問過的那張下一次又會被問一遍——一個看起來
    // 合理的數字把一句假話說得很順。這個 repo 已經為同一種寫法付過 67 次帳。
    //
    // 時間解不出來就退到這一列自己的 `updated_at`：那正是上次寫下這格的時刻，
    // 是同一件事的另一個量法，不是編的。
    let Ok(commitment_id) = id.parse::<i64>() else {
        return Ok(None);
    };
    Ok(Some(crate::followup::FollowupState {
        commitment_id,
        last_asked_at: at.parse().unwrap_or(row.updated_at),
    }))
}

pub fn record_followup(db: &mut Db, commitment_id: i64, now: Millis) -> Result<()> {
    db.upsert_preference(
        l3_write(),
        FOLLOWUP_PREFERENCE,
        &format!("{commitment_id}:{now}"),
        &format!("commitment:{commitment_id}"),
        now,
    )
}

pub fn close_from_message(
    db: &mut Db,
    message: &str,
    now: Millis,
) -> Result<crate::followup::CloseIntent> {
    let decision = crate::followup::resolve_close_intent(message, &db.live_commitments()?);
    if let crate::followup::CloseIntent::Close {
        commitment_id,
        ref kill_note,
    } = decision
    {
        kill_commitment(db, commitment_id, kill_note, now)?;
    }
    Ok(decision)
}

/// 使用者按「結案」。這是 status=dead 的真路徑。
pub fn kill_commitment(db: &mut Db, id: i64, note: &str, now: Millis) -> Result<u64> {
    db.update_commitment_status(l3_write(), id, "dead", Some(note), now)
}

/// 使用者按「其他一切」= snooze + 降權。這是 status=snoozed 的真路徑。
pub fn snooze_commitment(db: &mut Db, id: i64, now: Millis) -> Result<u64> {
    let n = db.update_commitment_status(l3_write(), id, "snoozed", None, now)?;
    if n > 0
        && let Some(c) = db.commitment_by_id(id)?
    {
        db.upsert_preference(
            l3_write(),
            &format!("snoozed_kind:{}", c.kind),
            "1",
            &format!("commitment:{id}"),
            now,
        )?;
    }
    Ok(n)
}

/// 當場更正 L2。author=user，下一輪不會蓋掉。
pub fn correct_l2(db: &mut Db, segment_core_start: Millis, activity: &str) -> Result<i64> {
    let prev = db
        .latest_l2_for_segment(segment_core_start)?
        .context("這一段還沒有假設可以改")?;
    db.insert_l2_card(&L2Insert {
        segment_core_start,
        segment_ref: &prev.segment_ref,
        activity,
        entities_json: prev.entities_json,
        continues_json: prev.continues_json,
        commitments_json: prev.commitments_json,
        model_confidence: prev.model_confidence,
        evidence_json: prev.evidence_json,
        open_questions_json: prev.open_questions_json,
        author: L2Author::User,
    })
}

fn write_day_summary(input: &mut ReviewInput<'_>, summarized_day: &str) -> Result<()> {
    // 審閱窗是 36 小時（`mark_done_from_originals` / `archive_overdue` 仍用那扇窗）。
    // 日摘要只收被盤點那一天的本地日界，不能把星期天晚上和星期二早上混進去。
    let (from_ts, to_ts) = brain::local_day_bounds(summarized_day)
        .with_context(|| format!("算不出 {summarized_day} 的本地日界，不敢寫日摘要"))?;
    let cards = input.db.l2_in_range(from_ts, to_ts)?;
    let mut latest: BTreeMap<Millis, L2CardRow> = BTreeMap::new();
    for row in cards {
        latest.insert(row.segment_core_start, row);
    }
    if latest.is_empty() {
        return Ok(());
    }
    let activities: Vec<String> = latest.values().map(|c| c.activity.clone()).collect();
    let refs: Vec<String> = latest.values().map(|c| format!("l2:{}", c.id)).collect();
    let narrative = activities.join("；");
    let stats = serde_json::json!({
        "l2": latest.len(),
        "commitments_open": input.db.live_commitments()?.iter().filter(|c| c.status == "open").count(),
    });
    input.db.insert_day_summary(
        l3_write(),
        &DaySummaryInsert {
            date: summarized_day,
            narrative: &narrative,
            session_refs_json: serde_json::to_string(&refs)?,
            stats_json: stats.to_string(),
            now: input.now,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{Sheet, VERSION};
    use crate::db::DaySummaryGlance;
    use crate::db::{ReviewerNotes, ReviewerRefusals};
    use crate::model::{FocusEvent, FocusKind, FocusSnapshot, FrameCapture, OcrBlock};
    use chrono::{Local, LocalResult, TimeZone};

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-reviewer-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("tmpdir");
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_cli(
        dir: &std::path::Path,
        json: &str,
        sentinel: &std::path::Path,
    ) -> (String, Vec<String>) {
        let script = dir.join("fake-reviewer.py");
        std::fs::write(
            &script,
            format!(
                "import sys, pathlib\nsys.stdin.buffer.read()\npathlib.Path(sys.argv[1]).write_text('spawned')\nsys.stdout.buffer.write({json:?}.encode('utf-8'))\n"
            ),
        )
        .expect("script");
        (
            "python3".into(),
            vec![
                script.to_string_lossy().into_owned(),
                sentinel.to_string_lossy().into_owned(),
            ],
        )
    }

    fn fake_cli_split(
        dir: &std::path::Path,
        json_a: &str,
        json_b: &str,
        sentinel: &std::path::Path,
    ) -> (String, Vec<String>) {
        let script = dir.join("fake-dual.py");
        std::fs::write(
            &script,
            format!(
                "import sys, pathlib\npayload = sys.stdin.buffer.read()\npathlib.Path(sys.argv[1]).write_bytes(b'spawned')\nbody = {json_b:?}.encode('utf-8') if b'PASS_B' in payload else {json_a:?}.encode('utf-8')\nsys.stdout.buffer.write(body)\n"
            ),
        )
        .expect("script");
        (
            "python3".into(),
            vec![
                script.to_string_lossy().into_owned(),
                sentinel.to_string_lossy().into_owned(),
            ],
        )
    }

    fn fake_cli_swaps_fact_after_prompt(
        dir: &std::path::Path,
        json: &str,
        db_path: &std::path::Path,
        fact_id: i64,
    ) -> (String, Vec<String>) {
        let script = dir.join("fake-swap-after-prompt.py");
        std::fs::write(
            &script,
            format!(
                "import sys, sqlite3\nsys.stdin.buffer.read()\nconn = sqlite3.connect(sys.argv[1])\nconn.execute(\"UPDATE facts SET raw = ? WHERE id = ?\", ('https://swapped.example/', int(sys.argv[2])))\nconn.commit()\nsys.stdout.buffer.write({json:?}.encode('utf-8'))\n"
            ),
        )
        .expect("script");
        (
            "python3".into(),
            vec![
                script.to_string_lossy().into_owned(),
                db_path.to_string_lossy().into_owned(),
                fact_id.to_string(),
            ],
        )
    }

    /// 「切法變了」出現在 [`CUT_CHANGED_AS_EXAMPLE`] 之外，就是把它講成主句。
    /// 不能再用「『例如』有沒有出現在它前面」：那會把另一段的「例如」當成通行證。
    fn cut_changed_appears_outside_the_example(s: &str) -> bool {
        s.replace(CUT_CHANGED_AS_EXAMPLE, "").contains("切法變了")
    }

    fn expected_still_there_line(segment_ref: &str, covering_clause: &str) -> String {
        let cap_min = crate::segment::TIME_CAP_MS / 60_000;
        let uncomputed_example = if covering_clause.is_empty() {
            "，或那段範圍還沒算過"
        } else {
            ""
        };
        format!(
            "{MISSING_SEGMENT_NOTE_HEAD}（{segment_ref}）現在查不到（{CUT_CHANGED_AS_EXAMPLE}{uncomputed_example}）；那張卡片的起點之後{cap_min}分鐘內還留著紀錄{covering_clause}，所以這一輪沒有給模型任何畫面上的 fact，也沒有替它解下一步。"
        )
    }

    /// 同一份輸出裡，同一個標籤詞不可以配上兩個不同輪次。
    fn assert_no_shared_label_across_rounds(shown: &str) {
        use std::collections::{BTreeMap, BTreeSet};
        let markers = [
            (
                "最近一次試著問模型的審閱",
                "最近一次試著問模型的審閱（輪次 #",
            ),
            ("最近一次問過模型的審閱", "最近一次問過模型的審閱（輪次 #"),
            ("最近一次雙 pass", "最近一次雙 pass（審閱輪次 #"),
            ("最近一次審閱", "最近一次審閱（輪次 #"),
        ];
        let mut by_label: BTreeMap<&str, BTreeSet<i64>> = BTreeMap::new();
        for (label, marker) in markers {
            let mut rest = shown;
            while let Some(at) = rest.find(marker) {
                let after = &rest[at + marker.len()..];
                let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                let id: i64 = digits
                    .parse()
                    .unwrap_or_else(|_| panic!("標籤「{label}」後面不是輪次數字：{shown}"));
                by_label.entry(label).or_default().insert(id);
                rest = after;
            }
        }
        for (label, ids) in &by_label {
            assert!(
                ids.len() <= 1,
                "「{label}」配上了兩個不同輪次 {ids:?}：{shown}"
            );
        }
    }

    fn wipe_raw_records_in_core_window(db: &Db, core: Millis) {
        let raw_to = core.saturating_add(crate::segment::TIME_CAP_MS);
        // facts 先於 text_chunks：chunk_id 是 ON DELETE CASCADE，順序反過來
        // 那句 DELETE 的 rowcount 會變成假的零。
        for (sql, what) in [
            ("DELETE FROM facts WHERE ts >= ?1 AND ts < ?2", "facts"),
            (
                "DELETE FROM text_chunks WHERE ts >= ?1 AND ts < ?2",
                "text_chunks",
            ),
            ("DELETE FROM frames WHERE ts >= ?1 AND ts < ?2", "frames"),
            (
                "DELETE FROM focus_events WHERE ts >= ?1 AND ts < ?2",
                "focus_events",
            ),
            (
                "DELETE FROM clipboard_events WHERE ts >= ?1 AND ts < ?2",
                "clipboard",
            ),
            (
                "DELETE FROM input_metrics WHERE ts_end > ?1 AND ts_start < ?2",
                "input_metrics",
            ),
            (
                "DELETE FROM system_events WHERE ts >= ?1 AND ts < ?2",
                "system_events",
            ),
            ("DELETE FROM queries WHERE ts >= ?1 AND ts < ?2", "queries"),
            (
                "DELETE FROM segment_edit WHERE from_ms IS NOT NULL AND ((to_ms IS NOT NULL AND from_ms < ?2 AND to_ms > ?1) OR (to_ms IS NULL AND from_ms >= ?1 AND from_ms < ?2))",
                "segment_edit",
            ),
        ] {
            db.conn.execute(sql, [core, raw_to]).expect(what);
        }
    }

    fn seed(db: &mut Db, ts: Millis, ocr: &str) -> (i64, i64) {
        let sid = db.start_session("test", "0").expect("session");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts: ts + 180_000,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus b");
        let frame = FrameCapture {
            ts: ts + 30_000,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: ocr.into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        (sid, fid)
    }

    fn signed() -> Consent {
        let mut c = Consent::default();
        c.grant(Sheet::CloudReading, 1);
        c
    }

    fn local_ms(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> Option<Millis> {
        match Local.with_ymd_and_hms(year, month, day, hour, min, sec) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => Some(dt.timestamp_millis()),
            LocalResult::None => None,
        }
    }

    fn write_l2(db: &mut Db, core: Millis, fid: i64, activity: &str, commits: &str) -> i64 {
        db.insert_l2_card(&L2Insert {
            segment_core_start: core,
            segment_ref: &format!("segment:{core}"),
            activity,
            entities_json: r#"[{"type":"person","name":"王小明"}]"#.into(),
            continues_json: None,
            commitments_json: commits.into(),
            model_confidence: 0.62,
            evidence_json: format!(r#"["frame:{fid}"]"#),
            open_questions_json: r#"["還沒存檔"]"#.into(),
            author: L2Author::Interpreter,
        })
        .expect("l2")
    }

    #[test]
    fn two_kinds_of_zero_are_not_the_same_sentence() {
        let never = RecheckStats {
            runs: None,
            candidates: None,
            rechecks: None,
            last_skip: None,
        };
        let ran_none = RecheckStats {
            runs: Some(3),
            candidates: Some(0),
            rechecks: Some(0),
            last_skip: None,
        };
        let ran_no_look = RecheckStats {
            runs: Some(3),
            candidates: Some(5),
            rechecks: Some(0),
            last_skip: None,
        };
        let a = format_recheck_rate(&never);
        let b = format_recheck_rate(&ran_none);
        let c = format_recheck_rate(&ran_no_look);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert!(a.contains("還沒跑過"), "{a}");
        assert!(b.contains("沒有五類"), "{b}");
        assert!(c.contains("一次都沒回查"), "{c}");
    }

    #[test]
    fn visibility_copy_names_both_passes_and_entity_mentions() {
        use crate::db::{DivergenceRow, EntityRow, EntityWithMentions, MentionRow};

        let divergence = DualPassDivergences::Diverged {
            run_id: 7,
            rows: vec![DivergenceRow {
                id: 1,
                run_id: 7,
                subject: "l2:42 / 交報告".into(),
                pass_a_json: r#"{"stands":true}"#.into(),
                pass_b_json: r#"{"stands":false}"#.into(),
                reason: "stands 不同".into(),
                created_at: 20,
            }],
        };
        let entities = EntityMemory::Present(vec![EntityWithMentions {
            entity: EntityRow {
                id: 2,
                kind: "project".into(),
                name: "AI-Sister".into(),
                aliases_json: "[]".into(),
                first_seen_ref: "l2:42".into(),
                notes: None,
                created_at: 20,
                tombstoned_at: None,
            },
            mentions: vec![MentionRow {
                id: 3,
                entity_id: 2,
                seen_ref: "l2:42".into(),
                created_at: 20,
                tombstoned_at: None,
            }],
        }]);
        let text = format_reviewer_visibility(
            &divergence,
            &ReviewerRefusals::NeverRan,
            &ReviewerNotes::NeverRan,
            &entities,
        );
        for expected in [
            "l2:42 / 交報告",
            "pass A：{\"stands\":true}",
            "pass B：{\"stands\":false}",
            "[project] AI-Sister",
            "出現於：l2:42",
        ] {
            assert!(text.contains(expected), "缺少 {expected:?}：{text}");
        }
    }

    #[test]
    fn visibility_copy_keeps_all_six_states_apart() {
        use crate::db::{DivergenceRow, EntityRow, EntityWithMentions};

        // 型別分成三段，不代表**螢幕上**分成三句。db.rs 那邊證明的是
        // 「挑對了 variant」；這裡證明的是「三個 variant 講三句不一樣的話」。
        // 少了這一條，把 `NeverRan` 的字串改成 `Agreed` 的字串，全綠。
        let row = DivergenceRow {
            id: 1,
            run_id: 7,
            subject: "l2:42".into(),
            pass_a_json: "{}".into(),
            pass_b_json: "{}".into(),
            reason: "stands 不同".into(),
            created_at: 20,
        };
        let entity = EntityWithMentions {
            entity: EntityRow {
                id: 2,
                kind: "project".into(),
                name: "AI-Sister".into(),
                aliases_json: "[]".into(),
                first_seen_ref: "l2:42".into(),
                notes: None,
                created_at: 20,
                tombstoned_at: None,
            },
            mentions: vec![],
        };

        let d = [
            DualPassDivergences::NeverRan,
            DualPassDivergences::NoComparableAnswers {
                run_id: 7,
                rows: vec![row.clone()],
            },
            DualPassDivergences::Agreed { run_id: 7 },
            DualPassDivergences::Diverged {
                run_id: 7,
                rows: vec![row],
            },
        ]
        .map(|s| {
            format_reviewer_visibility(
                &s,
                &ReviewerRefusals::NeverRan,
                &ReviewerNotes::NeverRan,
                &EntityMemory::NeverReviewed,
            )
        });
        let e = [
            EntityMemory::NeverReviewed,
            EntityMemory::GotNoAnswer,
            EntityMemory::Empty,
            EntityMemory::Present(vec![entity]),
        ]
        .map(|s| {
            format_reviewer_visibility(
                &DualPassDivergences::NeverRan,
                &ReviewerRefusals::NeverRan,
                &ReviewerNotes::NeverRan,
                &s,
            )
        });

        for (label, texts) in [("分歧", &d), ("實體", &e)] {
            for i in 0..texts.len() {
                for j in (i + 1)..texts.len() {
                    assert_ne!(
                        texts[i], texts[j],
                        "{label}的第 {i} 與第 {j} 種狀態印出同一句話"
                    );
                }
            }
        }

        // 「幾句話不一樣」還不夠。「沒有分歧」是一句**比較過**才講得出口的話，
        // 只有 Agreed 有資格講；NeverRan 沒有兩份答案可比，NoComparableAnswers
        // 試過但沒拿到答案，Diverged 剛好相反。少了這幾條，把 NeverRan 改成印
        // 「沒有分歧。」照樣幾句不同、照樣全綠。
        assert!(d[0].contains("還沒跑過"), "{}", d[0]);
        assert!(!d[0].contains("沒有分歧"), "還沒比就說沒有分歧：{}", d[0]);
        assert!(
            d[1].contains("沒有拿到可比較的兩份答案"),
            "試過、沒問到要講出來：{}",
            d[1]
        );
        assert!(!d[1].contains("的分歧"), "沒有兩份答案卻說分歧：{}", d[1]);
        assert!(!d[1].contains("沒有分歧"), "沒問到卻說沒有分歧：{}", d[1]);
        assert!(d[2].contains("沒有分歧"), "{}", d[2]);
        assert!(!d[3].contains("沒有分歧"), "有分歧卻說沒有：{}", d[3]);

        assert!(e[0].contains("還沒跑過"), "{}", e[0]);
        assert!(
            !e[0].contains("沒有活著的實體"),
            "沒跑過卻說辨識到 0 個：{}",
            e[0]
        );
        assert!(
            e[1].contains("沒拿到答案"),
            "試過、沒問到要講出來：{}",
            e[1]
        );
        assert!(
            !e[1].contains("沒有活著的實體"),
            "沒問到卻說辨識到 0 個：{}",
            e[1]
        );
        assert!(e[2].contains("沒有活著的實體"), "{}", e[2]);
        assert!(!e[3].contains("沒有活著的實體"), "有實體卻說沒有：{}", e[3]);
    }

    #[test]
    fn never_asked_the_model_is_not_the_same_sentence_as_asked_and_refused_nothing() {
        let never = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &ReviewerRefusals::NeverRan,
            &ReviewerNotes::NeverRan,
            &EntityMemory::NeverReviewed,
        );
        let none = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &ReviewerRefusals::None { run_id: 1 },
            &ReviewerNotes::NeverRan,
            &EntityMemory::NeverReviewed,
        );
        assert_ne!(
            never, none,
            "還沒問過和問過、沒拒絕印成同一句：\nnever={never}\nnone={none}"
        );
        assert!(
            never.contains("還沒問過模型"),
            "NeverRan 要講還沒問過：{never}"
        );
        // 釘的是「有沒有回答『她拒絕了什麼』」這件事，不是隔壁兩臂的字面值。
        // 「沒有資格回答」＝拒絕回答這個問題；「沒有拒絕」＝給了答案。
        // 把 NeverRan 改成「所以這一輪沒有拒絕」時，隔壁的字面針穿過，這一條要紅。
        assert!(
            never.contains("沒有資格回答"),
            "還沒問過必須拒絕回答她拒絕了什麼，不能改口給一個答案：{never}"
        );
        assert!(
            !never.contains("沒有拒絕"),
            "還沒問過卻宣告她沒有拒絕：{never}"
        );
        assert!(
            none.contains("沒有拒絕任何下一步"),
            "問過、沒拒絕要講出來：{none}"
        );
    }

    #[test]
    fn visibility_says_out_loud_what_it_did_not_list() {
        use crate::db::{EntityRow, EntityWithMentions, MentionRow};

        // 用了幾週之後，實體會有幾百個、單一實體的提及會有幾百段。
        // 截斷本身沒問題，**默默**截斷才有問題：一份被剪掉尾巴的清單
        // 讀起來跟一份完整的清單長得一模一樣。
        let rows: Vec<EntityWithMentions> = (0..40)
            .map(|i| EntityWithMentions {
                entity: EntityRow {
                    id: i,
                    kind: "person".into(),
                    name: format!("人{i:02}"),
                    aliases_json: "[]".into(),
                    first_seen_ref: "l2:1".into(),
                    notes: None,
                    created_at: 20,
                    tombstoned_at: None,
                },
                mentions: (0..30)
                    .map(|m| MentionRow {
                        id: i * 100 + m,
                        entity_id: i,
                        seen_ref: format!("l2:{i}-{m}"),
                        created_at: 20,
                        tombstoned_at: None,
                    })
                    .collect(),
            })
            .collect();
        let text = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &ReviewerRefusals::NeverRan,
            &ReviewerNotes::NeverRan,
            &EntityMemory::Present(rows),
        );

        assert!(text.contains("共 40 個"), "沒說總共幾個：{text}");
        assert!(
            text.contains("還有 20 個實體沒列出來"),
            "沒說漏了幾個實體：{text}"
        );
        assert!(!text.contains("人39"), "第 40 個不該印出來：{text}");
        assert!(
            text.contains("另有 24 段沒列出來"),
            "沒說單一實體漏了幾段：{text}"
        );
        assert!(!text.contains("l2:0-29"), "第 30 段不該印出來：{text}");
    }

    #[test]
    fn mentions_are_counted_by_segment_not_by_row() {
        use crate::db::{EntityRow, EntityWithMentions, MentionRow};

        // `entity_mentions` 上沒有 `(entity_id, seen_ref)` 的 UNIQUE，而同一張
        // 卡每輪會插兩列（承諾裡的人 + 卡上的實體），`latest_unreviewed` 又不會
        // 排除審過的卡——所以同一段會累積幾十上百列。「段」是這一行自己的宣稱，
        // 數列不等於數段。
        let rows = vec![EntityWithMentions {
            entity: EntityRow {
                id: 1,
                kind: "person".into(),
                name: "王小明".into(),
                aliases_json: "[]".into(),
                first_seen_ref: "l2:87".into(),
                notes: None,
                created_at: 20,
                tombstoned_at: None,
            },
            mentions: (0..140)
                .map(|i| MentionRow {
                    id: i,
                    entity_id: 1,
                    seen_ref: "l2:87".into(),
                    created_at: 20,
                    tombstoned_at: None,
                })
                .collect(),
        }];
        let text = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &ReviewerRefusals::NeverRan,
            &ReviewerNotes::NeverRan,
            &EntityMemory::Present(rows),
        );
        assert!(
            text.contains("出現於：l2:87\n"),
            "同一段印了不只一次：{text}"
        );
        assert!(!text.contains("沒列出來"), "一段被數成 140 段：{text}");
    }

    #[test]
    fn visibility_stays_quiet_when_nothing_was_cut() {
        use crate::db::{EntityRow, EntityWithMentions, MentionRow};

        // 反面：沒剪過就不准出現「沒列出來」，否則下一個人會學會忽略它。
        let rows = vec![EntityWithMentions {
            entity: EntityRow {
                id: 1,
                kind: "person".into(),
                name: "Ted".into(),
                aliases_json: "[]".into(),
                first_seen_ref: "l2:1".into(),
                notes: None,
                created_at: 20,
                tombstoned_at: None,
            },
            mentions: vec![MentionRow {
                id: 1,
                entity_id: 1,
                seen_ref: "l2:1".into(),
                created_at: 20,
                tombstoned_at: None,
            }],
        }];
        let text = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &ReviewerRefusals::NeverRan,
            &ReviewerNotes::NeverRan,
            &EntityMemory::Present(rows),
        );
        assert!(!text.contains("沒列出來"), "沒剪卻在講剪：{text}");
        assert!(text.contains("Ted"), "{text}");
    }

    #[test]
    fn skip_messages_are_not_the_same_sentence() {
        let msgs = [
            SkipReason::NoConsent.message(),
            SkipReason::NoCommand.message(),
            SkipReason::BudgetExhausted {
                used: 40,
                limit: 40,
            }
            .message(),
            SkipReason::Cadence {
                last_ago_ms: 60_000,
                min_ms: MIN_INTERVAL_MS,
            }
            .message(),
            SkipReason::NothingToReview { remaining: 40 }.message(),
        ];
        for (i, x) in msgs.iter().enumerate() {
            for (j, y) in msgs.iter().enumerate() {
                if i != j {
                    assert_ne!(x, y, "兩種原因印成同一句話");
                }
            }
        }
    }

    /// 「沒有東西可審」寫進 notes 不是 detail。手捏 `ReviewerRefusals` 再叫
    /// render 函式證不到這一格——要走真正的 `run`，讓它自己把那句話寫進哪一欄。
    /// 沒看過卡片的一輪沒有資格說「沒有拒絕任何下一步」。
    #[test]
    fn an_empty_interval_review_must_not_claim_she_refused_nothing() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_800_000_000;
        let consent = signed();
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec![],
            ..Default::default()
        };
        let result = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts,
                to_ts: ts + 400_000,
                kind: ReviewKind::Interval,
                force: true,
                now: ts + 500_000,
            };
            run(&mut input).expect("run")
        };
        assert!(
            matches!(result.skip, Some(SkipReason::NothingToReview { .. })),
            "這一條要走的是沒有東西可審，不是別種跳過：{:?}",
            result.skip
        );
        assert!(result.ran, "看過了、沒有東西，要記成一次真正跑過");

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );

        assert!(
            !shown.contains("沒有拒絕任何下一步"),
            "沒看過卡片的一輪沒有資格說「沒有拒絕」：{shown}"
        );
        assert!(
            !shown.contains("拒絕掉的下一步"),
            "空的區間審閱不准把「沒有東西」講成拒絕：{shown}"
        );
        let Some(notes_at) = shown.find("另外記下的說明：") else {
            panic!("那句話要出現在說明那一段：{shown}");
        };
        assert!(
            shown[notes_at..].contains("這段期間沒有還沒審過的 L2 假設"),
            "那句話要出現在說明那一段底下：{shown}"
        );
        if let Some(refusals_at) = shown.find("拒絕掉的下一步") {
            let refusals_end = shown[refusals_at..]
                .find("另外記下的說明：")
                .map(|i| refusals_at + i)
                .unwrap_or(shown.len());
            assert!(
                !shown[refusals_at..refusals_end].contains("這段期間沒有還沒審過的 L2 假設"),
                "那句話不可以出現在拒絕那一段：{shown}"
            );
        }
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("沒有東西可審要留下一則說明");
        };
        assert_eq!(lines.len(), 1, "一則說明不能被 \\n 裁成兩則：{lines:?}");
    }

    /// 閒置的一輪沒有資格回答「有沒有拒絕」。上一輪真的拒絕過的，
    /// `sister review --dry-run` 仍要看得到，而且不准改口說「沒有拒絕任何下一步」。
    #[test]
    fn an_idle_tick_does_not_erase_the_previous_round_s_refusals() {
        let tmp = Tmp::new("idle-does-not-erase");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        let fact_ts = ts + crate::segment::TIME_CAP_MS + 30_000;
        let fact_id = db
            .test_insert_fact(fact_ts, "url", "https://window.example/")
            .expect("fact");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{fact_id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let first = run_diverge(&mut db, command.clone(), args.clone(), ts);
        assert_eq!(first.refused_next_steps, 1, "第一輪要真的拒絕過");
        let ReviewerRefusals::Some { reasons, .. } = db.latest_reviewer_refusals().unwrap() else {
            panic!("第一輪必須留下拒絕");
        };
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&format!("fact:{fact_id}"))),
            "{reasons:?}"
        );

        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let second_now = ts + 500_000 + MIN_INTERVAL_MS + 1;
        let second = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts + 10_000_000,
                to_ts: ts + 10_400_000,
                kind: ReviewKind::Interval,
                force: false,
                now: second_now,
            };
            run(&mut input).expect("idle run")
        };
        assert!(
            matches!(second.skip, Some(SkipReason::NothingToReview { .. })),
            "第二輪要走沒有東西可審：{:?}",
            second.skip
        );
        assert!(second.ran, "看過了、沒有東西，要記成一次真正跑過");

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            shown.contains(&format!("fact:{fact_id}")),
            "閒置 tick 之後上一輪的拒絕還要看得見：{shown}"
        );
        assert!(
            shown.contains("拒絕掉的下一步"),
            "閒置 tick 不准把上一輪的拒絕清單蓋掉：{shown}"
        );
        assert!(
            !shown.contains("沒有拒絕任何下一步"),
            "閒置 tick 不准改口說沒有拒絕：{shown}"
        );
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("第二輪要留下「沒有東西可審」的說明");
        };
        assert_eq!(lines.len(), 1, "一則說明不能被裁成兩則：{lines:?}");
        assert!(
            lines[0].contains("這段期間沒有還沒審過的 L2 假設"),
            "{lines:?}"
        );
        assert!(
            shown.contains("最近一次問過模型的審閱"),
            "拒絕那一塊要講它真正取的那一輪：{shown}"
        );
        assert_no_shared_label_across_rounds(&shown);
    }

    /// 日終沒有候選時，和閒置 tick 同一個洞：`candidate_count = 0`、
    /// `skip_reason = NULL`、`notes = ""`。對不上任何文案開頭，只能看那個數字。
    #[test]
    fn an_empty_eod_does_not_erase_the_previous_round_s_refusals() {
        let tmp = Tmp::new("eod-does-not-erase");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        let fact_ts = ts + crate::segment::TIME_CAP_MS + 30_000;
        let fact_id = db
            .test_insert_fact(fact_ts, "url", "https://window.example/")
            .expect("fact");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{fact_id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let first = run_diverge(&mut db, command.clone(), args.clone(), ts);
        assert_eq!(first.refused_next_steps, 1, "第一輪要真的拒絕過");
        let ReviewerRefusals::Some { reasons, .. } = db.latest_reviewer_refusals().unwrap() else {
            panic!("第一輪必須留下拒絕");
        };
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&format!("fact:{fact_id}"))),
            "{reasons:?}"
        );

        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let second = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts + 10_000_000,
                to_ts: ts + 10_400_000,
                kind: ReviewKind::Eod,
                force: false,
                now: ts + 500_000 + 1,
            };
            run(&mut input).expect("empty eod")
        };
        assert_eq!(second.candidates, 0, "第二輪區間裡沒有卡片：{second:?}");
        assert!(second.ran, "沒有候選的日終仍記成一次真正跑過");

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            shown.contains(&format!("fact:{fact_id}")),
            "沒有候選的日終之後上一輪的拒絕還要看得見：{shown}"
        );
        assert!(
            shown.contains("拒絕掉的下一步"),
            "沒有候選的日終不准把上一輪的拒絕清單蓋掉：{shown}"
        );
        assert!(
            !shown.contains("沒有拒絕任何下一步"),
            "沒有候選的日終不准改口說沒有拒絕：{shown}"
        );
        assert!(
            shown.contains("最近一次問過模型的審閱"),
            "拒絕那一塊要講它真正取的那一輪：{shown}"
        );
        assert_no_shared_label_across_rounds(&shown);
    }

    /// 「最近」落在 `ORDER BY ts DESC, id DESC` 上。只有一輪看得了卡片的
    /// 夾具分不出來；三輪——拒絕 A、夾一輪沒有候選的、拒絕 B——才分得出來。
    #[test]
    fn latest_reviewer_refusals_is_the_latest_card_looking_round() {
        let tmp = Tmp::new("latest-is-latest");
        let mut db = Db::open_in_memory().expect("db");
        let ts_a = 1_700_250_000_000;
        let fact_a = refuse_a_step_at(&mut db, &tmp, ts_a, "https://first.example/");
        let consent = signed();
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec![],
            ..Default::default()
        };
        let idle_now = ts_a + 500_000 + MIN_INTERVAL_MS + 1;
        {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts_a + 10_000_000,
                to_ts: ts_a + 10_400_000,
                kind: ReviewKind::Interval,
                force: false,
                now: idle_now,
            };
            let idle = run(&mut input).expect("idle");
            assert!(
                matches!(idle.skip, Some(SkipReason::NothingToReview { .. })),
                "中間那一輪要走沒有東西可審：{:?}",
                idle.skip
            );
        }
        let ts_b = ts_a + 20_000_000;
        let fact_b = refuse_a_step_at(&mut db, &tmp, ts_b, "https://second.example/");
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            shown.contains(&format!("fact:{fact_b}")),
            "畫面上要是最近那一輪的拒絕 B：{shown}"
        );
        assert!(
            !shown.contains(&format!("fact:{fact_a}")),
            "ORDER BY 反過來會停在第一輪的拒絕 A：{shown}"
        );
    }

    /// 跳過的一列 `skip_reason` 不是 NULL、`detail` 有內容，不可以變成畫面上
    /// 的「拒絕掉的下一步」。`record_skip` 寫 `calls_used = 0`，新判準
    /// `calls_used > 0` 也擋得住；這一條把 `calls_used` 寫成 2，專門守
    /// `WHERE skip_reason IS NULL` 自己——少了它，預算用完會被印成一句拒絕。
    #[test]
    fn a_skipped_reviewer_run_must_not_become_a_refusal_on_screen() {
        let (mut db, first) =
            run_next_step_fixture("skip-not-refusal", "LINE：五點去接她 17:00", 999_999, None);
        assert_eq!(first.refused_next_steps, 1, "第一輪要真的拒絕一步");
        let skip_detail = SkipReason::BudgetExhausted {
            used: 40,
            limit: 40,
        }
        .message();
        db.insert_reviewer_run(&ReviewerRunInsert {
            ts: 1_700_250_000_000 + 1_000_000,
            day_key: "2023-11-17",
            kind: "interval",
            skip_reason: Some("budget"),
            candidate_count: None,
            recheck_count: None,
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 2,
            budget_used: 40,
            budget_limit: 40,
            detail: &skip_detail,
            notes: "",
            answers_got: None,
        })
        .expect("skip row");
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            shown.contains("999999"),
            "跳過的一輪把上一輪真的拒絕擦掉了：{shown}"
        );
        assert!(
            shown.contains("拒絕掉的下一步"),
            "跳過的一輪不准把拒絕清單蓋掉：{shown}"
        );
        assert!(
            !shown.contains("今天的審閱預算已用完"),
            "跳過的一輪被印成拒絕掉的下一步：{shown}"
        );
    }

    #[test]
    fn unsigned_consent_does_not_spawn() {
        let tmp = Tmp::new("noconsent");
        let sentinel = tmp.0.join("spawned");
        let json = r#"{"commitments":[]}"#;
        let (command, args) = fake_cli(&tmp.0, json, &sentinel);
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_100_000_000;
        let (_sid, fid) = seed(&mut db, ts, "五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let consent = Consent::default();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            kind: ReviewKind::Interval,
            force: true,
            now: ts + 500_000,
        };
        let result = run(&mut input).expect("run");
        assert!(matches!(result.skip, Some(SkipReason::NoConsent)));
        assert!(!sentinel.exists(), "沒簽卻 spawn 了");
        assert!(db.live_commitments().expect("c").is_empty());
    }

    #[test]
    fn agreeing_dual_pass_writes_an_open_commitment() {
        let tmp = Tmp::new("agree");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_200_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00 王小明");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":["王小明"],"confidence":0.8,"evidence_refs":["frame:{fid}"]}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            kind: ReviewKind::Interval,
            force: true,
            now: ts + 500_000,
        };
        let result = run(&mut input).expect("run");
        assert!(result.skip.is_none(), "{:?}", result.skip);
        assert!(result.rechecks > 0, "有五類卻沒回查");
        assert_eq!(result.wrote_commitments, 1);
        let cs = db.live_commitments().expect("c");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].status, "open");
        assert_eq!(cs[0].due_source.as_deref(), Some("explicit"));
        assert_eq!(cs[0].text, "五點去接她");
        let orig = db
            .l0_original(&EvidenceRef::Frame(fid))
            .expect("orig")
            .expect("present");
        assert!(orig.text.contains("五點去接她"), "回查讀到的應是 OCR 原文");
        assert!(!orig.text.contains("在看接人的訊息") || orig.text.contains("LINE"));
    }

    #[test]
    fn existing_frame_not_shown_to_model_is_dropped_from_evidence() {
        let tmp = Tmp::new("unshown-existing-frame");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_225_000_000;
        let (_sid, shown_fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let (_other_sid, unshown_fid) = seed(&mut db, ts + 1_000_000, "另一張真的存在的畫面");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{shown_fid}","frame:{unshown_fid}"]}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            shown_fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.wrote_commitments, 1);
        let row = db
            .live_commitments()
            .expect("commitments")
            .pop()
            .expect("one");
        let refs: Vec<String> = serde_json::from_str(&row.evidence_json).expect("evidence json");
        assert!(refs.contains(&format!("frame:{shown_fid}")));
        assert!(
            !refs.contains(&format!("frame:{unshown_fid}")),
            "模型沒看過、但真的存在的 frame ref 不得寫進資料庫：{refs:?}"
        );
    }

    fn run_evidence_filter_fixture(
        label: &str,
        evidence_refs: impl FnOnce(i64, i64, i64) -> Vec<String>,
    ) -> (Vec<String>, Vec<String>, ReviewResult, i64, i64, i64) {
        let tmp = Tmp::new(label);
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_230_000_000;
        let (_sid, shown_fid) = seed(&mut db, ts, "LINE：五點去接她 17:00 https://shown.example/");
        let (_other_sid, unshown_fid) = seed(&mut db, ts + 1_000_000, "另一張真的存在的畫面");
        let shown_fact = fact_id(&db, ts, "url", "https://shown.example/");
        let refs = evidence_refs(shown_fid, unshown_fid, shown_fact);
        let response = serde_json::json!({
            "commitments": [{
                "text": "五點去接她",
                "stands": true,
                "kind": "promise",
                "due_hint": "17:00",
                "due_source": "explicit",
                "people": [],
                "confidence": 0.8,
                "evidence_refs": refs
            }]
        })
        .to_string();
        let (command, args) = fake_cli(&tmp.0, &response, &sentinel);
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            shown_fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        let row = db
            .live_commitments()
            .expect("commitments")
            .pop()
            .expect("one");
        let stored = serde_json::from_str(&row.evidence_json).expect("evidence json");
        let agreed = match row.agreed_evidence_json.as_deref() {
            None => panic!("新寫入的承諾不該把 agreed_evidence_json 留成 NULL"),
            Some(json) => serde_json::from_str(json).expect("agreed evidence json"),
        };
        (stored, agreed, result, shown_fid, unshown_fid, shown_fact)
    }

    #[test]
    fn nonexistent_frame_is_dropped_from_evidence() {
        let (refs, _, _, _, _, _) =
            run_evidence_filter_fixture("nonexistent-frame", |_, _, _| vec!["frame:9999".into()]);
        assert!(!refs.contains(&"frame:9999".to_string()));
    }

    #[test]
    fn non_reference_string_is_dropped_from_evidence() {
        let (refs, _, _, _, _, _) =
            run_evidence_filter_fixture("non-ref", |_, _, _| vec!["這不是一個 ref".into()]);
        assert!(!refs.contains(&"這不是一個 ref".to_string()));
    }

    #[test]
    fn original_reference_shown_to_model_remains_in_evidence() {
        let (refs, _, _, shown_fid, _, _) =
            run_evidence_filter_fixture("shown-original", |shown_fid, _, _| {
                vec![format!("frame:{shown_fid}")]
            });
        assert!(refs.contains(&format!("frame:{shown_fid}")));
    }

    #[test]
    fn fact_reference_shown_to_model_remains_in_evidence() {
        let (refs, _, _, _, _, shown_fact) =
            run_evidence_filter_fixture("shown-fact", |_, _, shown_fact| {
                vec![format!("fact:{shown_fact}")]
            });
        assert!(refs.contains(&format!("fact:{shown_fact}")));
    }

    #[test]
    fn dropped_evidence_reference_count_matches_removed_refs() {
        let (_, _, result, _, _, _) =
            run_evidence_filter_fixture("drop-count", |shown, unseen, _| {
                vec![
                    format!("frame:{shown}"),
                    format!("frame:{unseen}"),
                    "frame:9999".into(),
                    "這不是一個 ref".into(),
                ]
            });
        assert_eq!(result.dropped_evidence_refs, 3);
    }

    #[test]
    fn agreed_evidence_is_filtered_by_the_same_shown_set_without_double_counting() {
        let (union, agreed, result, shown_fid, unshown_fid, _) =
            run_evidence_filter_fixture("agreed-retain", |shown, unseen, _| {
                vec![format!("frame:{shown}"), format!("frame:{unseen}")]
            });
        let shown = format!("frame:{shown_fid}");
        let unshown = format!("frame:{unshown_fid}");
        assert!(union.contains(&shown), "{union:?}");
        assert!(
            !union.contains(&unshown),
            "聯集裡不該留下沒給模型看過的 ref：{union:?}"
        );
        assert!(agreed.contains(&shown), "{agreed:?}");
        assert!(
            !agreed.contains(&unshown),
            "交集若沒過同一道濾網，會出現聯集裡沒有、交集裡卻有的 ref：{agreed:?}"
        );
        assert!(
            agreed.iter().all(|r| union.contains(r)),
            "交集必須是聯集的子集：union={union:?} agreed={agreed:?}"
        );
        assert_eq!(result.dropped_evidence_refs, 1);
    }

    #[test]
    fn dropping_evidence_refs_does_not_count_as_refused_next_steps() {
        let (_, _, result, _, _, _) =
            run_evidence_filter_fixture("drop-not-refusal", |_, _, _| vec!["frame:9999".into()]);
        assert_eq!(result.refused_next_steps, 0);
        assert!(result.dropped_evidence_refs > 0);
    }

    #[test]
    fn review_result_only_mentions_dropped_evidence_when_nonzero() {
        let (_, _, mut result, _, _, _) =
            run_evidence_filter_fixture("drop-display", |_, _, _| vec!["frame:9999".into()]);
        let stats = RecheckStats {
            runs: Some(1),
            candidates: Some(1),
            rechecks: Some(1),
            last_skip: None,
        };
        let shown = format_review_result(&result, &stats);
        assert!(shown.contains("不代表承諾被拒絕"), "{shown}");
        result.dropped_evidence_refs = 0;
        let quiet = format_review_result(&result, &stats);
        assert!(!quiet.contains("根本沒給模型看過"), "{quiet}");
    }

    fn run_next_step_fixture(
        label: &str,
        ocr: &str,
        fact_id: i64,
        pass_b_fact_id: Option<i64>,
    ) -> (Db, ReviewResult) {
        let tmp = Tmp::new(label);
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, ocr);
        if label == "next-url-scheme" {
            let inserted = db
                .test_insert_fact(ts + 30_000, "url", "example.com/x")
                .expect("fact");
            assert_eq!(inserted, fact_id, "測試注入的 fact id 漂移");
        }
        let card = |next: Option<i64>| {
            let next = next
                .map(|id| format!(r#"{{"fact":{id}}}"#))
                .unwrap_or_else(|| "null".into());
            format!(
                r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{next}}}]}}"#
            )
        };
        let json_a = card((fact_id >= 0).then_some(fact_id));
        let json_b = card(pass_b_fact_id.or((fact_id >= 0).then_some(fact_id)));
        let (command, args) = fake_cli_split(&tmp.0, &json_a, &json_b, &sentinel);
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        (db, result)
    }

    fn fact_id(db: &Db, ts: Millis, kind: &str, raw: &str) -> i64 {
        db.facts_in_range(ts, ts + 400_000)
            .expect("facts")
            .into_iter()
            .find(|f| f.kind == kind && f.raw == raw)
            .unwrap_or_else(|| panic!("找不到 {kind} fact {raw:?}"))
            .id
    }

    #[test]
    fn target_fact_whose_row_was_swapped_is_refused() {
        let mut db = Db::open_in_memory().expect("db");
        let id = db
            .test_insert_fact(1_700_250_000_000, "url", "https://attacker.example/")
            .expect("fact");
        let mut originally_listed = db.fact_by_id(id).expect("query").expect("listed fact");
        originally_listed.raw = "https://original.example/".into();

        let other = db
            .test_insert_fact(1_700_250_000_001, "url", "https://other.example/")
            .expect("other fact");
        let other = db.fact_by_id(other).expect("query").expect("other listed");
        let result = resolve_allowed_next_step(
            &db,
            &[other, originally_listed],
            Some(&NextStepRef { fact: id }),
        )
        .expect("resolve");
        let ResolvedNextStep::Refused(reason) = result else {
            panic!("row 換人後仍然解出下一步");
        };
        assert!(
            reason.contains("已經不是當初列給模型看的那一列"),
            "{reason}"
        );
    }

    #[test]
    fn real_review_pass_refuses_a_fact_swapped_after_prompt_was_built() {
        let tmp = Tmp::new("swap-after-prompt");
        let db_path = tmp.0.join("sister.db");
        let mut db = Db::open(&db_path).expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(
            &mut db,
            ts,
            "LINE：五點去接她 17:00 https://first.example/ https://target.example/",
        );
        let facts = db.facts_in_range(ts, ts + 400_000).expect("listed facts");
        let target = facts
            .iter()
            .find(|fact| fact.raw == "https://target.example/")
            .expect("target fact");
        assert_ne!(facts.first().expect("two facts").id, target.id);
        let response = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{}}}}}]}}"#,
            target.id
        );
        let (command, args) =
            fake_cli_swaps_fact_after_prompt(&tmp.0, &response, &db_path, target.id);
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.wrote_commitments, 1, "承諾本身應保留");
        assert_eq!(result.refused_next_steps, 1, "換列的下一步必須被拒絕");
        let row = db
            .live_commitments()
            .expect("commitments")
            .pop()
            .expect("one");
        assert!(row.allowed_next_step.is_none());
        assert!(row.allowed_next_step_fact.is_none());
        let ReviewerRefusals::Some { reasons, .. } =
            db.latest_reviewer_refusals().expect("refusals")
        else {
            panic!("真 review pass 沒留下拒絕理由");
        };
        assert!(
            reasons
                .iter()
                .any(|reason| reason.contains("當初列給模型看的"))
        );
    }

    #[test]
    fn a_url_fact_becomes_an_open_url_next_step() {
        let ocr = "LINE：五點去接她 17:00 https://example.com/x";
        let mut seeded = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        seed(&mut seeded, ts, ocr);
        let id = fact_id(&seeded, ts, "url", "https://example.com/x");
        let (db, _) = run_next_step_fixture("next-url", ocr, id, None);
        assert_eq!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .as_deref(),
            Some(r#"{"action":"open_url","url":"https://example.com/x"}"#)
        );
    }

    /// 三條視窗測試共用同一份資料，**只差目標 fact 的時間**。
    ///
    /// `where_fact` 拿到 `(core, core_end)` 再決定放哪裡：有一條要釘的是
    /// 「剛好越過接縫 1 毫秒」，那個時間點得先知道 `core_end` 才算得出來。
    fn run_segment_window_next_step(
        label: &str,
        where_fact: impl FnOnce(Millis, Millis) -> Millis,
    ) -> (Db, ReviewResult, Millis, Millis, i64) {
        let tmp = Tmp::new(label);
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        let core_end = segs[0].core_ended_at;
        let fact_ts = where_fact(core, core_end);
        let fact_id = db
            .test_insert_fact(fact_ts, "url", "https://window.example/")
            .expect("fact");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{fact_id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        (db, result, core, core_end, fact_id)
    }

    #[test]
    fn a_fact_inside_this_segment_becomes_the_next_step() {
        let ts = 1_700_250_000_000;
        let fact_ts = ts + 30_000;
        let (db, result, core, core_end, _) =
            run_segment_window_next_step("segwin-inside", |_, _| fact_ts);
        assert!(
            fact_ts >= core && fact_ts < core_end,
            "這一條的 fact 必須落在這一段之內：fact_ts={fact_ts} core={core} core_end={core_end}"
        );
        assert_eq!(result.refused_next_steps, 0);
        assert_eq!(result.cards_missing_segment, 0, "這一段明明還在");
        assert_eq!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .as_deref(),
            Some(r#"{"action":"open_url","url":"https://window.example/"}"#)
        );
    }

    #[test]
    fn a_fact_outside_this_segment_but_inside_one_hour_is_refused() {
        let ts = 1_700_250_000_000;
        let fact_ts = ts + 1_800_000;
        let (db, result, core, core_end, fact_id) =
            run_segment_window_next_step("segwin-outside", |_, _| fact_ts);
        assert!(
            fact_ts >= core_end,
            "這一條的 fact 必須在這一段之外：fact_ts={fact_ts} core_end={core_end}"
        );
        assert!(
            fact_ts < core.saturating_add(3_600_000),
            "這一條的 fact 必須仍在舊的一小時窗內，否則證不到是視窗在起作用：fact_ts={fact_ts} core={core}"
        );
        assert!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .is_none()
        );
        assert_refused(
            &db,
            &result,
            &[&format!("fact:{fact_id}"), "沒有列在這次給模型的 L1 facts"],
        );
    }

    /// 接縫上的那一毫秒。**這一條專門釘住「不加 `OVERLAP_MARGIN_MS`」**。
    ///
    /// 上面那條把 fact 放在半小時外，右界就算多寬 5 秒也照樣拒絕——它證不到
    /// margin 這件事。core 邊界是精確相接的（`segment.rs` 的「核心邊界該相接」），
    /// 所以 `core_end` 之後的第一毫秒**已經屬於下一段**。授權視窗伸過去，
    /// 就是這次要修的那個 bug 的縮小版。
    #[test]
    fn a_fact_one_millisecond_past_the_seam_belongs_to_the_next_segment() {
        let (db, result, core, core_end, fact_id) =
            run_segment_window_next_step("segwin-seam", |_, core_end| core_end + 1);
        let fact_ts = core_end + 1;
        assert!(
            fact_ts > core && fact_ts < core_end.saturating_add(crate::segment::OVERLAP_MARGIN_MS),
            "這一條的 fact 必須落在 margin 之內，否則證不到 margin：\
             fact_ts={fact_ts} core={core} core_end={core_end}"
        );
        assert!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .is_none(),
            "接縫外一毫秒的 fact 不可以變成下一步"
        );
        assert_eq!(result.cards_missing_segment, 0, "這一段還在，不是查不到");
        assert_refused(
            &db,
            &result,
            &[&format!("fact:{fact_id}"), "沒有列在這次給模型的 L1 facts"],
        );
    }

    #[test]
    fn a_missing_segment_refuses_even_a_fact_inside_one_hour() {
        let tmp = Tmp::new("segwin-missing");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        // 只刪 segment：快取沒了，畫面還在。這一條要的是 fail-closed
        // （查不到那一段也不准退回一小時），不是「窗裡什麼都沒有」那一臂。
        // 後者見 `the_gone_branch_does_not_blame_a_cause_it_cannot_prove`。
        // 不要在這個窗裡再 `test_insert_fact` 一筆——那筆是真的原始紀錄，
        // 分辨法會看到它。目標 fact 放在 TIME_CAP 窗外、一小時內。
        assert!(
            db.has_raw_records_in_core_window(core).expect("raw"),
            "畫面還在，分辨法該走「還留著紀錄」"
        );
        assert_eq!(
            db.segment_core_end(core).expect("query"),
            None,
            "這一條要的是查不到那一段，不是讀失敗"
        );
        let fact_ts = ts + crate::segment::TIME_CAP_MS + 30_000;
        assert!(
            fact_ts < core.saturating_add(3_600_000),
            "目標 fact 仍在一小時內：查不到那一段若退回一小時，這一條會放行"
        );
        assert!(
            fact_ts >= core.saturating_add(crate::segment::TIME_CAP_MS),
            "目標 fact 要在 TIME_CAP 窗外，不然 test_insert_fact 會讓分辨法看到一筆紀錄"
        );
        let fact_id = db
            .test_insert_fact(fact_ts, "url", "https://window.example/")
            .expect("fact");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{fact_id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .is_none(),
            "查不到那一段還解出下一步"
        );
        // **這兩個數字要分開對，而且要對死。** 原本這裡寫的是
        // `refused_next_steps >= 1`，而實際跑出來是 2——模型只指了一步，
        // 「她拒絕了 N 個模型指的下一步」卻會印 2。`>=` 把它遮住了。
        assert_eq!(
            result.refused_next_steps, 1,
            "模型只指了一步，拒絕計數就只能是 1"
        );
        assert_eq!(
            result.cards_missing_segment, 1,
            "段落不見了要算在自己那一格，不是算成「模型指的下一步」"
        );
        let ReviewerRefusals::Some { reasons, .. } = db.latest_reviewer_refusals().unwrap() else {
            panic!("查不到那一段必須留下理由");
        };
        // 兩句話要同時在，而且**不可以是同一句**。拒絕那句仍在 `detail`；
        // 「這一段不見了」不是拒絕，改看 `notes`。
        assert!(
            reasons
                .iter()
                .any(|r| r.contains(&format!("fact:{fact_id}"))
                    && r.contains("沒有列在這次給模型的 L1 facts")),
            "少了「模型指的那筆 fact 不在清單裡」那一句：{reasons:?}"
        );
        assert!(
            !reasons.iter().any(|r| r.contains("這張卡片指的那一段")),
            "「這一段不見了」不可以再混進拒絕清單：{reasons:?}"
        );
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("查不到那一段必須留下說明");
        };
        assert!(
            lines.iter().any(|r| r.contains("這張卡片指的那一段")
                && r.contains("沒有給模型任何畫面上的 fact")
                && r.contains("現在查不到")
                && r.contains("還留著紀錄")
                && r.contains("切法變了")
                && !cut_changed_appears_outside_the_example(r)),
            "畫面還在，主句只講查不到、還留著紀錄；切法變了只能當例子：{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|r| r.contains("被忘掉") || r.contains("保留期")),
            "畫面還在，不准點名忘掉或保留期：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("仍被另一段蓋著")),
            "快取是空的，附加子句不該出現：{lines:?}"
        );
        // 借 `TargetApp::Forgotten` 的字串會讓兩種狀態講同一句話：那一句講的是
        // 「**下一步目標**那筆 fact 的來源沒了」，這裡沒了的是「**卡片自己那一段**」。
        assert!(
            !reasons
                .iter()
                .any(|r| r.contains("這個目標的來源已經不在了")),
            "段落不見了不可以借用目標來源不見了的那句話：{reasons:?}"
        );
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(shown.contains("拒絕掉的下一步"), "{shown}");
        assert!(shown.contains("另外記下的說明"), "{shown}");
        assert!(!shown.contains("沒有拒絕任何下一步"), "{shown}");
    }

    /// 走到「窗裡什麼都沒有」那一臂時，那句話不准點名一個它證明不了的成因。
    ///
    /// 正式路徑通常走不到這一臂（血緣會把卡片一起墓碑），但
    /// `migrate_012` 不回填 provenance 的舊卡片落差是開著的。夾具是段落列
    /// 沒了、原始紀錄也沒了、卡片還活著。擴大涵蓋範圍之後，問過的每一張
    /// 表都要一起刪，不然會走到「還留著紀錄」那一臂。
    #[test]
    fn the_gone_branch_does_not_blame_a_cause_it_cannot_prove() {
        let tmp = Tmp::new("segwin-gone-no-cause");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        wipe_raw_records_in_core_window(&db, core);
        assert!(
            !db.has_raw_records_in_core_window(core).expect("raw"),
            "這一條要的是窗裡什麼紀錄都沒有"
        );
        assert_eq!(
            db.segment_core_end(core).expect("query"),
            None,
            "這一條要的是查不到那一段，不是讀失敗"
        );
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.cards_missing_segment, 1);
        assert!(
            db.live_commitments()
                .unwrap()
                .iter()
                .all(|c| c.allowed_next_step.is_none()),
            "這一臂仍然 fail-closed，一筆 fact 都不給"
        );
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("查不到那一段必須留下說明");
        };
        assert!(
            lines.iter().any(|r| r.contains("這張卡片指的那一段")
                && r.contains("現在查不到")
                && r.contains("沒有留下任何原始紀錄")
                && r.contains("分鐘內")
                && r.contains("沒有給模型任何畫面上的 fact")),
            "要講出連原始紀錄都沒有這個觀察到的事實，主詞是起點之後的窗：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("那段時間的紀錄")
                || r.contains("那段時間也沒有")
                || r.contains("那段時間還")),
            "程式界不出那一段，不可以用「那段時間」當主詞：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("被忘掉")
                || r.contains("保留期")
                || r.contains("forget")
                || r.contains("prune")),
            "不准點名一個它證明不了的成因：{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|r| r.contains("還留著紀錄") || r.contains("切法變了")),
            "窗裡沒有紀錄，不可以走「還留著紀錄」那一臂：{lines:?}"
        );
    }

    /// 句子裡的「N 分鐘內」必須和 probe 實際掃的窗是同一段時間。
    /// 造一列落在真正的 `TIME_CAP` 之後、硬編碼 30 分鐘之內的紀錄：
    /// 把 `cap_min` 釘死成 30、或把 probe 的窗放寬，句子和 probe 就會講相反。
    #[test]
    fn the_minutes_in_the_sentence_are_the_window_the_probe_scans() {
        let tmp = Tmp::new("cap-min-binds-probe");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        wipe_raw_records_in_core_window(&db, core);
        let planted_ts = core.saturating_add(crate::segment::TIME_CAP_MS) + 30_000;
        db.test_insert_fact(planted_ts, "url", "https://after-cap.example/")
            .expect("plant just after the real window");
        assert!(
            !db.has_raw_records_in_core_window(core).expect("raw"),
            "這一列要落在真正的窗之後，probe 才是 false"
        );
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.cards_missing_segment, 1);
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("查不到那一段必須留下說明");
        };
        let line = lines
            .iter()
            .find(|r| r.contains("那張卡片的起點之後") && r.contains("分鐘內"))
            .unwrap_or_else(|| panic!("說明要講起點之後 N 分鐘內：{lines:?}"));
        let marker = "那張卡片的起點之後";
        let after = line
            .split_once(marker)
            .map(|(_, rest)| rest)
            .unwrap_or(line);
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let claimed_min: i64 = digits
            .parse()
            .unwrap_or_else(|_| panic!("句子裡的分鐘數解析不出來：{line}"));
        let claimed_end = core.saturating_add(claimed_min.saturating_mul(60_000));
        let record_in_claimed = planted_ts >= core && planted_ts < claimed_end;
        let probe = db.has_raw_records_in_core_window(core).expect("probe");
        assert_eq!(
            probe, record_in_claimed,
            "句子裡的 {claimed_min} 分鐘和 probe 對同一列講相反：probe={probe} record_in_claimed={record_in_claimed} line={line}"
        );
        if probe {
            assert!(
                line.contains("還留著紀錄"),
                "probe 是 true，句子要說還留著紀錄：{line}"
            );
        } else {
            assert!(
                line.contains("沒有留下任何原始紀錄"),
                "probe 是 false，句子要說沒有留下任何原始紀錄：{line}"
            );
        }
    }

    /// 零拒絕的時候仍然印出「沒有拒絕任何下一步」。
    ///
    /// 卡片那一段不見了、模型沒指下一步：說明在另一塊，不准把「沒有拒絕」壓掉。
    #[test]
    fn a_run_with_missing_segment_and_no_next_step_still_says_it_refused_nothing() {
        let tmp = Tmp::new("segwin-missing-no-next");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.refused_next_steps, 0, "模型沒指下一步");
        assert_eq!(result.cards_missing_segment, 1);
        assert_eq!(
            db.latest_reviewer_refusals().unwrap(),
            ReviewerRefusals::None { run_id: 1 },
            "說明不可以把「沒有拒絕」壓成「拒絕了這幾個」"
        );
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("零拒絕也要留下「這一段不見了」的說明");
        };
        assert!(
            lines
                .iter()
                .any(|r| r.contains("這張卡片指的那一段")
                    && r.contains("沒有給模型任何畫面上的 fact")),
            "{lines:?}"
        );
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            shown.contains("沒有拒絕任何下一步"),
            "零拒絕必須把這句印出來：{shown}"
        );
        assert!(
            !shown.contains("拒絕掉的下一步"),
            "零拒絕不准印拒絕清單標題：{shown}"
        );
        assert!(shown.contains("另外記下的說明"), "說明要自己一塊：{shown}");
        assert!(
            shown.contains("最近一次問過模型的審閱"),
            "None 那一臂也要講它真正取的那一輪：{shown}"
        );
    }

    /// 摘要那一行和底下的說明不能互相打臉。兩臂共用 `cards_missing_segment`，
    /// 常見的那一臂是「現在查不到、起點之後還留著紀錄」——摘要若寫「不在紀錄裡」，
    /// 同一頁差四行就在說假話。斷言必須對著摘要＋可見度整份輸出，
    /// 分開看每一句都對是這個 bug 的本體。
    ///
    /// 摘要那一行必須**正向**釘住「現在查不到」；只把「已經不在紀錄裡」
    /// 列進黑名單的話，換成另一句一樣打臉的話仍會全綠。
    #[test]
    fn the_summary_line_must_not_contradict_the_note_below_it() {
        let tmp = Tmp::new("summary-vs-note");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.cards_missing_segment, 1);
        assert!(
            db.has_raw_records_in_core_window(core).expect("raw"),
            "這一條要踩到「還留著紀錄」那一臂"
        );

        let stats = db.reviewer_recheck_stats().unwrap();
        let summary = format_review_result(&result, &stats);
        let visibility = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        let shown = format!("{summary}{visibility}");

        assert!(
            shown.contains("給它們任何畫面上的 fact"),
            "整份輸出必須含摘要那一行：{shown}"
        );
        assert!(
            summary.contains("現在查不到"),
            "摘要必須用指定的那一句「現在查不到」，不是「不含某六個字」：{summary}"
        );
        assert!(
            shown.contains("還留著紀錄"),
            "這一條要踩到「還留著紀錄」那一臂：{shown}"
        );
        assert!(
            shown.contains("另外記下的說明："),
            "「理由見底下」要指得到說明那一塊：{shown}"
        );
        assert!(
            !(shown.contains("已經不在紀錄裡") && shown.contains("還留著紀錄")),
            "摘要說不在紀錄裡、底下說還留著紀錄：{shown}"
        );
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("查不到那一段必須留下說明");
        };
        assert!(
            !lines
                .iter()
                .any(|r| cut_changed_appears_outside_the_example(r)),
            "切法變了只能當例子，不能當主句：{lines:?}"
        );
    }

    /// 快取空的、從來沒人算過段落：兩個 probe 也推不出「切法變了」。
    /// 主句只能講看得到的，成因全部降級成舉例。
    #[test]
    fn an_uncomputed_cache_must_not_claim_the_cut_changed() {
        let tmp = Tmp::new("uncomputed-cache");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let sid = db.start_session("test", "0").expect("session");
        db.insert_focus(
            sid,
            &FocusEvent {
                ts,
                kind: FocusKind::Focus,
                snapshot: FocusSnapshot {
                    app_id: Some("code.exe".into()),
                    ..Default::default()
                },
            },
        )
        .expect("focus");
        let frame = FrameCapture {
            ts: ts + 1_000,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: "LINE：五點去接她 17:00".into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        assert_eq!(
            db.covering_segment_at(ts).expect("cover"),
            None,
            "沒算過段落，快取是空的"
        );
        assert_eq!(
            db.segment_core_end(ts).expect("core"),
            None,
            "沒有一段從這個時間點開始"
        );
        assert!(
            db.has_raw_records_in_core_window(ts).expect("raw"),
            "原始紀錄還在"
        );
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            &mut db,
            ts,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.cards_missing_segment, 1);
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("查不到那一段必須留下說明");
        };
        assert!(
            lines.iter().any(|r| r.contains("現在查不到")
                && r.contains("還留著紀錄")
                && r.contains("分鐘內")),
            "主句要講看得到的：查不到、起點之後還留著紀錄：{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|r| cut_changed_appears_outside_the_example(r)),
            "沒算過段落，不可以平述地說切法變了：{lines:?}"
        );
        assert_eq!(
            lines[0],
            expected_still_there_line(&format!("segment:{ts}"), ""),
            "主句必須是指定的那一句，成因只出現在括號裡：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("那段時間的紀錄")
                || r.contains("那段時間也沒有")
                || r.contains("那段時間還")),
            "程式界不出那一段，不可以用「那段時間」當主詞：{lines:?}"
        );
    }

    fn seed_two_chapters(db: &mut Db, ts: Millis, ocr: &str) -> (i64, Millis, Millis) {
        let sid = db.start_session("test", "0").expect("session");
        for (offset, app) in [
            (0, "code.exe"),
            (180_000, "chrome.exe"),
            (240_000, "chrome.exe"),
        ] {
            db.insert_focus(
                sid,
                &FocusEvent {
                    ts: ts + offset,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some(app.into()),
                        ..Default::default()
                    },
                },
            )
            .expect("focus");
        }
        let frame = FrameCapture {
            ts: ts + 30_000,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: ocr.into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        assert!(
            segs.len() >= 2,
            "要兩段才能合併：{} 段 {:?}",
            segs.len(),
            segs.iter()
                .map(|s| (s.core_started_at - ts, s.core_ended_at - ts))
                .collect::<Vec<_>>()
        );
        (fid, segs[0].core_started_at, segs[1].core_started_at)
    }

    #[test]
    fn a_merged_segment_is_covered_not_forgotten() {
        let tmp = Tmp::new("segwin-merged");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (fid, left, right) = seed_two_chapters(
            &mut db,
            ts,
            "LINE：五點去接她 17:00 https://window.example/",
        );
        write_l2(
            &mut db,
            right,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        db.merge_chapters(left, right, ts, ts + 400_000)
            .expect("merge");
        assert_eq!(
            db.segment_core_end(right).expect("query"),
            None,
            "右邊那一段的起點合併後不在了"
        );
        let covering = db.covering_segment_at(right).expect("cover");
        assert!(
            covering.is_some_and(|(start, end)| start < right && right < end),
            "合併後那個時間點仍被另一段蓋著：{covering:?}"
        );
        let fact_id = db
            .test_insert_fact(right + 1, "url", "https://window.example/")
            .expect("fact");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{fact_id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.cards_missing_segment, 1);
        assert!(
            db.live_commitments()
                .unwrap()
                .iter()
                .all(|c| c.allowed_next_step.is_none()),
            "蓋住也不准把視窗放到那一段去"
        );
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("範圍變了必須留下說明");
        };
        assert!(
            lines
                .iter()
                .any(|r| r.contains("仍被另一段蓋著") && r.contains("例如")),
            "只能講蓋住，成因只能當例子：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("被忘掉、或過了保留期")),
            "東西還在，不准叫人去翻 forget：{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|r| r.contains("被合併了") && !r.contains("例如")),
            "不准把合併寫成唯一成因：{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|r| r.contains("你合併過章節") && !r.contains("例如")),
            "不准把合併寫成唯一成因：{lines:?}"
        );
    }

    #[test]
    fn rerunning_review_after_a_merge_does_not_write_a_card_for_the_new_range() {
        let tmp = Tmp::new("segwin-merged-rerun");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (fid, left, right) = seed_two_chapters(&mut db, ts, "LINE：五點去接她 17:00");
        write_l2(
            &mut db,
            right,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        db.merge_chapters(left, right, ts, ts + 400_000)
            .expect("merge");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let first = run_diverge(&mut db, command.clone(), args.clone(), ts);
        assert_eq!(first.cards_missing_segment, 1);
        let before = db.l2_in_range(ts, ts + 400_000).expect("l2");
        let left_cards = before
            .iter()
            .filter(|c| c.segment_core_start == left)
            .count();
        let second = run_diverge(&mut db, command, args, ts);
        assert_eq!(second.cards_missing_segment, 1);
        let after = db.l2_in_range(ts, ts + 400_000).expect("l2");
        let left_cards_after = after
            .iter()
            .filter(|c| c.segment_core_start == left)
            .count();
        assert_eq!(
            left_cards_after, left_cards,
            "重跑 review 不會替現在那一段寫一張新卡片；跑前 {left_cards}、跑後 {left_cards_after}"
        );
    }

    /// 重跑 `sister review` 不會把舊承諾的 `agreed_evidence_json` 補上。
    /// 它會另寫一筆；舊的 NULL 原封不動。所以不准寫「重跑就會過」。
    #[test]
    fn rerunning_review_adds_a_new_commitment_and_leaves_the_old_null_agreed_evidence() {
        let tmp = Tmp::new("old-agreed-rerun");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let ocr = "LINE：五點去接她 17:00 https://example.com/x";
        let (_sid, fid) = seed(&mut db, ts, ocr);
        let id = fact_id(&db, ts, "url", "https://example.com/x");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        db.conn
            .execute(
                "INSERT INTO commitments(
                    text, kind, born_from, evidence_json, agreed_evidence_json, people_json,
                    status, confidence, allowed_next_step, allowed_next_step_fact,
                    created_at, updated_at
                 ) VALUES(
                    '五點去接她','promise',1,'[\"frame:2\"]',NULL,'[]',
                    'open',0.7,'{\"action\":\"open_url\",\"url\":\"https://example.com/x\"}',?1,
                    1,1
                 )",
                [id],
            )
            .expect("old commitment");
        let first = run_diverge(&mut db, command.clone(), args.clone(), ts);
        assert!(first.wrote_commitments >= 1, "{first:?}");
        let second = run_diverge(&mut db, command, args, ts);
        assert!(second.wrote_commitments >= 1, "{second:?}");
        let mut nulls = 0i64;
        let mut filled = 0i64;
        let mut stmt = db
            .conn
            .prepare("SELECT agreed_evidence_json FROM commitments WHERE tombstoned_at IS NULL")
            .expect("prep");
        let rows = stmt
            .query_map([], |r| r.get::<_, Option<String>>(0))
            .expect("query");
        for row in rows {
            match row.expect("row") {
                None => nulls += 1,
                Some(_) => filled += 1,
            }
        }
        assert!(
            nulls >= 1,
            "舊的那筆 agreed_evidence_json 仍是 NULL：nulls={nulls} filled={filled}"
        );
        assert!(
            filled >= 1,
            "重跑會多一筆新的：nulls={nulls} filled={filled}"
        );
    }

    /// SPEC 列的「forget 之後走真的不見了那一格」。
    ///
    /// 實測：forget 會把 L2 卡片也墓碑，`l2_in_range` 看不到它，reviewer
    /// 走不到 `segment_core_end == None` 那句話。窗裡什麼紀錄都沒有那一臂，
    /// 到得了的輸入是段落列被拿掉、原始紀錄也沒了、卡片還活著
    /// （`the_gone_branch_does_not_blame_a_cause_it_cannot_prove`）。
    #[test]
    fn forgetting_the_range_tombstones_the_card_so_reviewer_never_sees_it() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        db.forget(ts, ts + 400_000, None).expect("forget");
        assert!(
            db.l2_in_range(ts, ts + 400_000).expect("l2").is_empty(),
            "forget 之後卡片也被墓碑，reviewer 看不到它"
        );
        assert_eq!(
            db.segment_core_end(core).expect("query"),
            None,
            "段落列確實沒了"
        );
    }

    #[test]
    fn recomputing_from_mid_activity_still_has_raw_events_so_must_not_say_forgotten() {
        let tmp = Tmp::new("segwin-recompute");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let min = 60_000i64;
        let sid = db.start_session("test", "0").expect("session");
        for i in 0..=20 {
            db.insert_focus(
                sid,
                &FocusEvent {
                    ts: ts + i * min,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some("code.exe".into()),
                        ..Default::default()
                    },
                },
            )
            .expect("focus");
        }
        let frame = FrameCapture {
            ts: ts + 12 * min,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: "LINE：五點去接她 17:00".into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        let first = db
            .chapters_for_range(ts, ts + 20 * min)
            .expect("first window");
        assert!(
            first.len() >= 2,
            "長活動該被 10 分鐘上限切成多段：{} 段 {:?}",
            first.len(),
            first
                .iter()
                .map(|s| (s.core_started_at - ts, s.core_ended_at - ts))
                .collect::<Vec<_>>()
        );
        let later = first
            .iter()
            .find(|s| s.core_started_at >= ts + crate::segment::TIME_CAP_MS)
            .unwrap_or(&first[first.len() - 1]);
        let old_core = later.core_started_at;
        write_l2(
            &mut db,
            old_core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        // 窗從活動中間開始，離真起點超過 LOOKAROUND（5 分鐘）。
        let from = ts + 7 * min;
        let _ = db
            .chapters_for_range(from, ts + 20 * min)
            .expect("recompute");
        assert_eq!(
            db.segment_core_end(old_core).expect("query"),
            None,
            "舊的 core_started_at 那一列要消失，否則這一條沒踩到重算"
        );
        let covering = db.covering_segment_at(old_core).expect("cover");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let result = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts,
                to_ts: ts + 21 * min,
                kind: ReviewKind::Interval,
                force: true,
                now: ts + 22 * min,
            };
            run(&mut input).expect("run")
        };
        // 快取軸：舊的 10 分鐘切點那一列消失，而且沒有任何一段蓋住它。
        // 資料軸：focus 和 frame 都還在。講哪句話看資料軸，所以不可以說被忘掉。
        assert_eq!(
            covering,
            None,
            "快取軸仍是沒蓋住；若開始蓋得住，附加子句會出現「仍被另一段蓋著」。first={:?}",
            first
                .iter()
                .map(|s| (s.core_started_at - ts, s.core_ended_at - ts))
                .collect::<Vec<_>>()
        );
        assert!(
            db.has_raw_records_in_core_window(old_core).expect("raw"),
            "重算沒刪原始紀錄，這一條要踩到的就是這個"
        );
        assert_eq!(result.cards_missing_segment, 1);
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("查不到那一段必須留下說明");
        };
        assert!(
            lines.iter().any(|r| r.contains("還留著紀錄")
                && r.contains("切法變了")
                && r.contains("例如")
                && !cut_changed_appears_outside_the_example(r)),
            "原始事件還在，主句只講還留著紀錄；切法變了只能當例子：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("被忘掉、或過了保留期")),
            "重算之後事件還在，不可以說被忘掉：{lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|r| r.contains("你合併過章節") && !r.contains("例如")),
            "不准把合併寫成唯一成因：{lines:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("仍被另一段蓋著")),
            "這一格快取沒蓋住，附加子句不該出現：{lines:?}"
        );
    }

    /// 另一道重算：先用後面那一截窗種卡，再算整天。
    /// 這不是 SPEC 寫的那道食譜；若它走「被蓋住」，就證明那一格不必是合併。
    #[test]
    fn a_full_recompute_after_a_narrow_window_covers_the_planted_core() {
        let tmp = Tmp::new("segwin-recompute-full");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let min = 60_000i64;
        let sid = db.start_session("test", "0").expect("session");
        for i in 0..=20 {
            db.insert_focus(
                sid,
                &FocusEvent {
                    ts: ts + i * min,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some("code.exe".into()),
                        ..Default::default()
                    },
                },
            )
            .expect("focus");
        }
        let frame = FrameCapture {
            ts: ts + 16 * min,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: "LINE：五點去接她 17:00".into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        let narrow = db
            .chapters_for_range(ts + 10 * min, ts + 20 * min)
            .expect("narrow");
        let planted = narrow
            .iter()
            .find(|s| s.core_started_at > ts + 10 * min)
            .or(narrow.last());
        let Some(planted) = planted else {
            panic!(
                "窄窗沒切出後面那一節：{:?}",
                narrow
                    .iter()
                    .map(|s| (s.core_started_at - ts, s.core_ended_at - ts))
                    .collect::<Vec<_>>()
            );
        };
        let old_core = planted.core_started_at;
        write_l2(
            &mut db,
            old_core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let _ = db
            .chapters_for_range(ts, ts + 20 * min)
            .expect("full recompute");
        assert_eq!(
            db.segment_core_end(old_core).expect("query"),
            None,
            "整天重算後舊起點不該還在"
        );
        let covering = db.covering_segment_at(old_core).expect("cover");
        let Some((start, _)) = covering else {
            panic!(
                "整天重算後 old_core=+{} 沒被蓋住；narrow={:?}",
                old_core - ts,
                narrow
                    .iter()
                    .map(|s| (s.core_started_at - ts, s.core_ended_at - ts))
                    .collect::<Vec<_>>()
            );
        };
        assert_ne!(start, old_core);
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let result = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts,
                to_ts: ts + 21 * min,
                kind: ReviewKind::Interval,
                force: true,
                now: ts + 22 * min,
            };
            run(&mut input).expect("run")
        };
        assert_eq!(result.cards_missing_segment, 1);
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("範圍變了必須留下說明");
        };
        assert!(
            lines.iter().any(|r| r.contains("仍被另一段蓋著")),
            "整天重算走的是蓋住那一格：{lines:?} covering={covering:?}"
        );
        assert!(
            !lines.iter().any(|r| r.contains("被忘掉、或過了保留期")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_file_path_fact_becomes_an_open_file_next_step() {
        let ocr = r"LINE：五點去接她 17:00 C:\work\report.txt";
        let mut seeded = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        seed(&mut seeded, ts, ocr);
        let id = fact_id(&seeded, ts, "file_path", r"C:\work\report.txt");
        let (db, _) = run_next_step_fixture("next-file", ocr, id, None);
        assert_eq!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .as_deref(),
            Some(r#"{"action":"open_file","path":"C:\\work\\report.txt"}"#)
        );
    }

    /// 這一串字，寫的人在 sister-core，讀的人在 sister-hands，中間沒有型別。
    ///
    /// 上面那兩條各自拿一段寫死的字面值跟自己比對。欄位改個名、
    /// `deny_unknown_fields` 多擋掉一個欄位，兩條都還是綠的，而他按下去會拿
    /// 到「按鈕內容已經讀不懂，沒有動手」。這一條把真的寫進資料庫的那串字撿
    /// 起來，交給真正會讀它的那支 parser，再交給真正會擋它的那支 policy：她
    /// 端出來的按鈕，要讀得懂，而且要按得下去。
    #[test]
    fn the_next_step_this_crate_writes_is_one_the_hands_crate_can_read_and_will_allow() {
        for (label, ocr, kind, raw) in [
            (
                "next-url",
                "LINE：五點去接她 17:00 https://example.com/x",
                "url",
                "https://example.com/x",
            ),
            (
                "next-file",
                r"LINE：五點去接她 17:00 C:\work\report.txt",
                "file_path",
                r"C:\work\report.txt",
            ),
        ] {
            let mut seeded = Db::open_in_memory().expect("db");
            let ts = 1_700_250_000_000;
            seed(&mut seeded, ts, ocr);
            let id = fact_id(&seeded, ts, kind, raw);
            let (db, _) = run_next_step_fixture(label, ocr, id, None);
            let written = db.live_commitments().unwrap()[0]
                .allowed_next_step
                .clone()
                .unwrap_or_else(|| panic!("{label}：根本沒寫下一步"));

            // 讀得懂嗎。這是 hands 那一支真正的 parser，不是在這裡重寫一份。
            let button = sister_hands::SuggestionButton::parse_json(&written)
                .unwrap_or_else(|e| panic!("{label}：hands 讀不懂 {written}：{e}"));
            let action = button.press().snapshot();
            assert!(
                action.describe().contains(raw),
                "{label}：按鈕上的字沒有指回那筆 fact，寫進去的是 {written}",
            );

            // 按得下去嗎。不要端一顆按下去一定會被 target_policy 擋掉的按鈕。
            match action {
                sister_hands::ActionSnapshot::OpenUrl { url } => {
                    sister_hands::target_policy::validate_url(&url)
                        .unwrap_or_else(|e| panic!("{label}：{e}"));
                }
                sister_hands::ActionSnapshot::OpenFile { path } => {
                    sister_hands::target_policy::validate_file(&path)
                        .unwrap_or_else(|e| panic!("{label}：{e}"));
                }
                sister_hands::ActionSnapshot::FocusWindow { title } => {
                    panic!("{label}：reviewer 不該寫出聚焦視窗：{title}")
                }
            }
        }
    }

    /// 一次拒絕要看得見，**而且不可以被記成一次分歧**。
    ///
    /// 分歧的意思是「兩份答案對不上」，畫面上印的是「pass A：… pass B：…」。
    /// 兩個 pass 講了同一句話而她拒絕了它，記成分歧的話，讀的人會看到一句
    /// 「分歧」配上兩段一模一樣的 JSON——每一段都是真的，湊起來在說謊。
    fn assert_refused(db: &Db, result: &ReviewResult, needles: &[&str]) {
        assert_eq!(result.divergences, 0, "拒絕不是分歧");
        assert!(
            db.list_reviewer_divergences(10).unwrap().is_empty(),
            "拒絕不該寫進 reviewer_divergence"
        );
        assert_eq!(result.refused_next_steps, 1);
        let ReviewerRefusals::Some { reasons, .. } = db.latest_reviewer_refusals().unwrap() else {
            panic!("拒絕過的那一輪要說得出它拒絕了什麼");
        };
        for needle in needles {
            assert!(
                reasons.iter().any(|r| r.contains(needle)),
                "{needle} 不在 {reasons:?} 裡"
            );
        }
        // 而且那句話真的會被印出來給人看，不是只躺在資料庫裡。
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        for needle in needles {
            assert!(shown.contains(needle), "{needle} 沒有被印出來：{shown}");
        }
        assert!(!shown.contains("的分歧："), "不可以說成分歧：{shown}");
    }

    #[test]
    fn a_missing_fact_is_refused_and_the_reason_is_visible() {
        let (db, result) =
            run_next_step_fixture("next-missing", "LINE：五點去接她 17:00", 999_999, None);
        assert!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .is_none()
        );
        assert_refused(&db, &result, &["999999", "不存在"]);
    }

    #[test]
    fn a_money_fact_is_refused_and_the_reason_is_visible() {
        let ocr = "LINE：五點去接她 17:00 NT$500";
        let mut seeded = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        seed(&mut seeded, ts, ocr);
        let id = fact_id(&seeded, ts, "money", "NT$500");
        let (db, result) = run_next_step_fixture("next-money", ocr, id, None);
        assert!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .is_none()
        );
        assert_refused(&db, &result, &["money", "拒絕"]);
    }

    #[test]
    fn a_url_without_a_scheme_is_refused_and_the_reason_is_visible() {
        let ocr = "LINE：五點去接她 17:00 example.com/x";
        let mut seeded = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        seed(&mut seeded, ts, ocr);
        let id = seeded
            .test_insert_fact(ts + 30_000, "url", "example.com/x")
            .expect("fact");
        let (db, result) = run_next_step_fixture("next-url-scheme", ocr, id, None);
        assert!(
            db.live_commitments().unwrap()[0]
                .allowed_next_step
                .is_none()
        );
        assert_refused(&db, &result, &["http://", "https://"]);
    }

    #[test]
    fn disagreeing_next_steps_keep_the_commitment_and_record_the_disagreement() {
        let ocr = "LINE：五點去接她 17:00 https://example.com/a https://example.com/b";
        let mut seeded = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        seed(&mut seeded, ts, ocr);
        let a = fact_id(&seeded, ts, "url", "https://example.com/a");
        let b = fact_id(&seeded, ts, "url", "https://example.com/b");
        let (db, result) = run_next_step_fixture("next-diverge", ocr, a, Some(b));
        let commitments = db.live_commitments().unwrap();
        assert_eq!(commitments.len(), 1, "下一步分歧不該殺掉承諾");
        assert!(commitments[0].allowed_next_step.is_none());
        assert_eq!(result.divergences, 1);
        assert!(
            db.list_reviewer_divergences(10).unwrap()[0]
                .reason
                .contains("allowed_next_step")
        );
    }

    #[test]
    fn a_null_next_step_keeps_the_commitment_without_a_refusal() {
        let (db, result) = run_next_step_fixture("next-null", "LINE：五點去接她 17:00", -1, None);
        let commitments = db.live_commitments().unwrap();
        assert_eq!(commitments.len(), 1);
        assert!(commitments[0].allowed_next_step.is_none());
        assert_eq!(result.divergences, 0, "null 不是分歧");
        assert!(db.list_reviewer_divergences(10).unwrap().is_empty());
        // 「他本來就沒有下一步」不可以被講成「她拒絕了一個下一步」。
        // 這一條是這支測試名字裡的 `without_a_refusal` 那半——少了它，
        // 把 `NotAsked` 整個改成 `Refused` 也照樣全綠。
        assert_eq!(result.refused_next_steps, 0, "沒有人要求過任何事");
        assert_eq!(
            db.latest_reviewer_refusals().unwrap(),
            ReviewerRefusals::None { run_id: 1 },
            "這一輪不該留下任何一句拒絕的理由"
        );
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(!shown.contains("拒絕掉的下一步"), "{shown}");
    }

    #[test]
    fn diverging_dual_pass_does_not_write_l3() {
        let tmp = Tmp::new("diverge");
        let sentinel = tmp.0.join("spawned");
        let json_a = r#"{"commitments":[{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:1"]}]}"#;
        let json_b = r#"{"commitments":[{"text":"五點去接她","stands":false,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.2,"evidence_refs":["frame:1"]}]}"#;
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_300_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let json_a = json_a.replace("frame:1", &format!("frame:{fid}"));
        let json_b = json_b.replace("frame:1", &format!("frame:{fid}"));
        let (command, args) = fake_cli_split(&tmp.0, &json_a, &json_b, &sentinel);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        let l2 = write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            kind: ReviewKind::Interval,
            force: true,
            now: ts + 500_000,
        };
        let result = run(&mut input).expect("run");
        assert_eq!(result.wrote_commitments, 0, "分歧還寫進 L3");
        assert!(result.divergences > 0);
        assert!(db.live_commitments().expect("c").is_empty());
        let rows = db.list_reviewer_divergences(10).expect("div");
        assert!(!rows.is_empty());
        assert!(rows[0].reason.contains("stands") || rows[0].reason.contains("對不上"));
        // 一句「兩個 pass 對『五點去接她』講不一樣」查不動——他得知道是**哪一張
        // L2**。承諾原文會重複出現在很多天的很多張卡上，`subject` 少了這個
        // 前綴，警報就只剩下情緒。
        assert!(
            rows[0].subject.contains(&format!("l2:{l2}")),
            "分歧沒說是哪一張 L2：{}",
            rows[0].subject
        );
        assert!(
            rows[0].subject.contains("五點去接她"),
            "分歧沒說是哪一筆承諾：{}",
            rows[0].subject
        );
    }

    /// 上面那條走的是「兩邊都看到、欄位對不上」。另外兩種分歧各自走不同的
    /// 分支，也各自要說得出是哪一張 L2；底下兩條分別釘住它們。
    fn diverge_fixture(
        tmp: &Tmp,
        json_a: &str,
        json_b: &str,
    ) -> (Db, i64, String, Vec<String>, Millis) {
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_300_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let json_a = json_a.replace("frame:1", &format!("frame:{fid}"));
        let json_b = json_b.replace("frame:1", &format!("frame:{fid}"));
        let (command, args) = fake_cli_split(&tmp.0, &json_a, &json_b, &sentinel);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        let l2 = write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        (db, l2, command, args, ts)
    }

    /// 一輪**真的撈到卡片、卻一次模型都沒呼叫**的巡邏，不可以把上一輪真的
    /// 拒絕擦掉。
    ///
    /// 這是同一個洞的第三扇門：`candidates` 在三個 `continue` 之前就先加了，
    /// 所以「候選不是 0」不等於「這一輪問過模型」。這一條是我在看修法**之前**
    /// 寫的，斷言只打在使用者看得到的那段字上，不綁判準要換成哪個欄位。
    #[test]
    fn ted_a_round_that_never_asked_the_model_must_not_erase_the_refusals() {
        let ts = 1_700_250_000_000;
        let (mut db, first) =
            run_next_step_fixture("never-asked", "LINE：五點去接她 17:00", 999_999, None);
        assert_eq!(first.refused_next_steps, 1, "第一輪要真的拒絕一步");

        let before = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            before.contains("999999"),
            "第一輪的拒絕本來就要看得見：{before}"
        );

        // 第二輪的窗裡放一張卡片，它的 evidence 指著一筆不存在的原件。
        // 於是 `candidates` 會加上去，但 `originals.is_empty()` 讓它在呼叫
        // 模型之前就被跳過——`calls_used` 是 0、`detail` 是空的。
        let later = ts + 3_600_000;
        write_l2(
            &mut db,
            later + 1_000,
            424_242,
            "這張卡片的原件已經不在了",
            r#"[{"text":"隨便一句","stands":true,"kind":"promise","people":[],"confidence":0.5,"evidence_refs":["frame:424242"]}]"#,
        );

        let consent = signed();
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec![],
            ..Default::default()
        };
        let second = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: later,
                to_ts: later + 400_000,
                kind: ReviewKind::Eod,
                force: false,
                now: later + 500_000,
            };
            run(&mut input).expect("run")
        };
        // 前提：這一輪確實走到了「有候選、沒問模型」那個形狀。
        // 這兩條是在證明測試還打得到那個洞，不是在規定修法。
        assert!(second.skip.is_none(), "第二輪要真的跑過：{:?}", second.skip);
        assert!(
            second.candidates > 0,
            "這一輪要真的數到候選（不然這條測試打不到那個洞）"
        );
        assert_eq!(
            second.refused_next_steps, 0,
            "這一輪根本沒問過模型，不可能拒絕任何一步"
        );

        let after = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            after.contains("999999"),
            "一輪連模型都沒問的巡邏，把上一輪真的拒絕擦掉了：{after}"
        );
        assert!(
            !after.contains("沒有拒絕任何下一步"),
            "沒問過模型的一輪不可以回答「她有沒有拒絕過」這個問題：{after}"
        );
    }

    /// 快取裡真的有一段蓋住那個時間點的時候，「那段範圍還沒算過」已經被排除，
    /// 不准再當成可能的成因印出來；而「你合併過章節」「那段範圍被重新計算過」
    /// 這兩個使用者真的可以動手去查的成因，一個都不准掉。
    ///
    /// 為什麼要這一條：round 7 加了 `covering.is_some()` 這道抑制，但我把那
    /// 一行整個換回無條件之後，1245 條測試全綠——那個行為改動一條斷言都沒有。
    /// 同一批測試裡唯一比對整句的那一條，期望值是拿產品的同一個常數算出來的，
    /// 所以我把常數砍短（拿掉破折號後面那兩個成因）之後也全綠。
    ///
    /// 所以這一條刻意把那兩個成因寫成字面值，不經過任何常數——鏡子照不出
    /// 自己變短了。
    ///
    /// 我在看修法之前寫的，不規定用哪個判斷式做抑制。
    #[test]
    fn ted_a_covered_moment_must_not_offer_uncomputed_and_must_keep_both_actionable_causes() {
        let tmp = Tmp::new("ted-covered-moment");
        let sentinel = tmp.0.join("spawned");
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let min = 60_000i64;
        let sid = db.start_session("test", "0").expect("session");
        for i in 0..=20 {
            db.insert_focus(
                sid,
                &FocusEvent {
                    ts: ts + i * min,
                    kind: FocusKind::Focus,
                    snapshot: FocusSnapshot {
                        app_id: Some("code.exe".into()),
                        ..Default::default()
                    },
                },
            )
            .expect("focus");
        }
        let frame = FrameCapture {
            ts: ts + 16 * min,
            monitor: 0,
            width: 100,
            height: 100,
            dhash: 1,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: "LINE：五點去接她 17:00".into(),
                x: 0,
                y: 0,
                w: 10,
                h: 10,
                confidence: 1.0,
            }],
            focus: FocusSnapshot {
                app_id: Some("code.exe".into()),
                ..Default::default()
            },
        };
        let (fid, _, _) = db.insert_frame(sid, &frame, None, 0).expect("frame");
        let narrow = db
            .chapters_for_range(ts + 10 * min, ts + 20 * min)
            .expect("narrow");
        let planted = narrow
            .iter()
            .find(|s| s.core_started_at > ts + 10 * min)
            .or(narrow.last());
        let Some(planted) = planted else {
            panic!("窄窗沒切出後面那一節");
        };
        let old_core = planted.core_started_at;
        write_l2(
            &mut db,
            old_core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let _ = db
            .chapters_for_range(ts, ts + 20 * min)
            .expect("full recompute");

        // 前提：整天重算之後，舊起點查不到了，但那個時間點仍被另一段蓋著。
        // 這兩條是在證明測試打得到那個洞，不是在規定修法。
        assert_eq!(
            db.segment_core_end(old_core).expect("query"),
            None,
            "舊起點不該還在"
        );
        let Some((start, _)) = db.covering_segment_at(old_core).expect("cover") else {
            panic!("那個時間點沒被蓋住，這條測試打不到那個洞");
        };
        assert_ne!(start, old_core, "蓋住它的必須是另一段");

        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":null}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let result = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts,
                to_ts: ts + 21 * min,
                kind: ReviewKind::Interval,
                force: true,
                now: ts + 22 * min,
            };
            run(&mut input).expect("run")
        };
        assert_eq!(result.cards_missing_segment, 1);
        let ReviewerNotes::Some { lines, .. } = db.latest_reviewer_notes().unwrap() else {
            panic!("範圍變了必須留下說明");
        };
        let line = lines
            .iter()
            .find(|r| r.contains("現在查不到"))
            .unwrap_or_else(|| panic!("那一句不見了：{lines:?}"));

        assert!(
            line.contains("仍被另一段蓋著"),
            "前提：走的要是「被蓋住」那一支：{line}"
        );
        assert!(
            !line.contains("還沒算過"),
            "那個時間點明明被另一段蓋著，快取算過了——把「還沒算過」列成成因會叫他去重算一段已經算過的範圍：{line}"
        );
        // 字面值，刻意不經過任何常數：鏡子照不出自己變短了。
        assert!(
            line.contains("合併過章節"),
            "使用者真的能動手的成因掉了一個（去時間軸看自己合併過什麼）：{line}"
        );
        assert!(
            line.contains("重新計算過"),
            "使用者真的能動手的成因掉了一個（那段範圍被重算過）：{line}"
        );
    }

    /// 拒絕那一塊的標籤，必須對它真的挑中的那一輪成立。
    ///
    /// round 7 把判準從「候選不是 0」換成「這一輪問過模型」（`calls_used`），
    /// 可是標籤那兩句字沒跟著動，還寫著「最近一次**看過卡片**的審閱」。
    /// 於是「第 1 輪問過模型、第 2 輪也看過卡片但一次都沒問」的機器上，
    /// 螢幕會指著第 1 輪說它是最近一次看過卡片的——第 2 輪才是。
    /// 同一個 `match` 裡 round 7 自己新加的那一臂用的是「還沒**問過模型**」，
    /// 三句話兩套講法、一個判準。
    ///
    /// 這一條是我在看修法**之前**寫的。我不規定查詢用哪個欄位，只要求那句
    /// 標籤講的是它真的拿來挑列的那件事——而那個詞產品裡已經有了，就是
    /// 隔壁那一臂的「問過模型」。
    #[test]
    fn ted_the_refusal_label_must_be_true_of_the_round_it_names() {
        let ts = 1_700_250_000_000;
        let (mut db, first) =
            run_next_step_fixture("refusal-label", "LINE：五點去接她 17:00", 999_999, None);
        assert_eq!(first.refused_next_steps, 1, "第一輪要真的拒絕一步");

        // 第二輪：窗裡有卡片（所以「看過卡片」對它成立），但它的原件不在，
        // 於是在呼叫模型之前就被跳過——`calls_used` 是 0。
        let later = ts + 3_600_000;
        write_l2(
            &mut db,
            later + 1_000,
            424_242,
            "這張卡片的原件已經不在了",
            r#"[{"text":"隨便一句","stands":true,"kind":"promise","people":[],"confidence":0.5,"evidence_refs":["frame:424242"]}]"#,
        );
        let consent = signed();
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec![],
            ..Default::default()
        };
        let second = {
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: later,
                to_ts: later + 400_000,
                kind: ReviewKind::Eod,
                force: false,
                now: later + 500_000,
            };
            run(&mut input).expect("run")
        };
        // 前提：第二輪確實看過卡片、確實沒問模型。證明這條測試打得到那個洞。
        assert!(second.skip.is_none(), "第二輪要真的跑過：{:?}", second.skip);
        assert!(second.candidates > 0, "第二輪要真的看過卡片");
        assert_eq!(second.refused_next_steps, 0, "第二輪沒問過模型");

        let after = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().unwrap(),
            &db.latest_reviewer_refusals().unwrap(),
            &db.latest_reviewer_notes().unwrap(),
            &db.entity_memory().unwrap(),
        );
        assert!(
            after.contains("999999"),
            "第一輪真的拒絕過的那一步還是要看得見：{after}"
        );
        let head = after
            .lines()
            .find(|l| l.contains("拒絕掉的下一步"))
            .unwrap_or_else(|| panic!("拒絕那一塊的標題不見了：{after}"));
        assert!(
            !head.contains("看過卡片"),
            "它指的是第一輪，但第二輪也看過卡片——這個標籤對它指的那一輪不成立：{head}"
        );
        assert!(
            head.contains("問過模型"),
            "判準問的是「有沒有問過模型」，標籤就要講這件事（隔壁那一臂已經這樣講了）：{head}"
        );
    }

    /// 叫不起那支 CLI 的一輪，不可以被說成「她沒有拒絕任何下一步」。
    ///
    /// `calls += 2` 在 `std::thread::scope` **後面、無條件**執行，而
    /// `spawn_cli` 在 `Command::spawn()` 失敗時**不回 `Err`**，回一個
    /// `spawn_error: Some("叫不起 …")` 的空 `SpawnOutcome`。所以 `calls_used`
    /// 數的是**試了幾次**，不是**問到幾次**。
    ///
    /// 於是 `brain.command` 指到一個不存在的執行檔時，那一輪
    /// `skip_reason IS NULL`、`calls_used = 2`、`detail` 是空的——挑列的判準
    /// （`calls_used > 0`）把它選中，畫面印出「最近一次問過模型的審閱
    /// （輪次 #N）沒有拒絕任何下一步。」。一個模型行程都沒起來。
    ///
    /// 同一份改動自己在 `entity_memory` 頭上把這個判準翻成「有沒有**開過**
    /// CLI」，在拒絕那一格頭上翻成「沒問過模型」——兩句話不是同一件事，
    /// 而使用者看到的是後面那句。
    ///
    /// 我在看修法之前寫的。兩個方向都收：判準改成只數真的問到的那幾次，
    /// 或那句標籤改成講它真的問得出來的事。不收的是替一輪零答案的跑，
    /// 宣告「她沒有拒絕任何下一步」——那句話正是 `NeverRan` 那一臂
    /// 特地不肯講的（「目前沒有資格回答她拒絕了什麼」）。
    #[test]
    fn ted_a_round_whose_cli_never_started_must_not_be_called_a_round_that_refused_nothing() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );

        // 這個名字不會存在於 PATH 上；`Command::spawn()` 一定失敗。
        let result = run_diverge(
            &mut db,
            "sister-no-such-brain-binary-9d3f".into(),
            vec![],
            ts,
        );

        // 前提三道：它真的跑過、真的把「試過」記成呼叫、而且一份答案都沒拿到。
        assert!(result.skip.is_none(), "這一輪要真的跑過：{:?}", result.skip);
        assert!(
            result.calls_used > 0,
            "前提變了：叫不起 CLI 的一輪不再記成有呼叫，這條測試要重寫"
        );
        assert_eq!(
            result.wrote_commitments, 0,
            "一個模型行程都沒起來，不該寫出承諾"
        );

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().expect("divergences"),
            &db.latest_reviewer_refusals().expect("refusals"),
            &db.latest_reviewer_notes().expect("notes"),
            &db.entity_memory().expect("entities"),
        );

        // 「審閱」出現在分歧那一塊的標題裡，守不住拒絕那一段。
        assert!(
            shown.contains("審閱") || shown.contains("拒絕"),
            "這一塊還是要有話講：{shown}"
        );
        assert!(
            shown.contains("沒拿到模型的答案"),
            "拒絕那一塊還是要有話講（擋『靠刪掉整段來過』）：{shown}"
        );
        assert!(
            !shown.contains("沒有拒絕任何下一步"),
            "一個模型行程都沒起來的一輪，不可以被端成「她沒有拒絕任何下一步」：{shown}"
        );
    }

    /// 螢幕上那一行預算，要跟真的被扣掉的那幾次一致。
    ///
    /// 真正的每日額度**不是** `calls_used` 在算的——是
    /// `brain_outbound_count_on_role(day, "reviewer")` 去數 `brain_outbound`
    /// 那張表的列（`reviewer.rs` 開頭那個 `used`）。而 `log_outbound` 對
    /// **每一次 spawn 都寫一列**，叫不起來的那一次寫成
    /// `OutboundOutcome::SpawnFailed`，一樣佔一列。
    ///
    /// 也就是說：**一輪叫不起 CLI 的跑，真的花掉了今天的兩次額度。**
    ///
    /// 所以「只在 `spawn_error.is_none()` 的時候才 `calls += 2`」那種修法會生出
    /// 一句新的假話：這一輪的畫面說「這一輪沒呼叫模型（預算還剩 40/40）」，
    /// 而下一輪的畫面會說「今日審閱呼叫 2/40」——中間沒有任何一輪承認花掉那兩次。
    /// 那正是這個分支在修的病換一扇門走回來。
    ///
    /// 我在看修法之前寫的。**這一條在 round 8 上是綠的**，它是迴歸網不是 bug 報告：
    /// 不管「問過模型」那三塊最後怎麼判，畫面上那行預算要繼續等於 outbound
    /// 紀錄真的記下的次數。要改這個對應關係的話，改的是 `log_outbound`
    /// 那一端（連同它的稽核意義），不是偷偷讓畫面少報。
    #[test]
    fn ted_the_budget_line_must_charge_what_the_outbound_log_charged() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );

        // 這個名字不會存在於 PATH 上；`Command::spawn()` 一定失敗。
        let result = run_diverge(
            &mut db,
            "sister-no-such-brain-binary-9d3f".into(),
            vec![],
            ts,
        );

        let day = brain::local_day_key(ts).expect("day key");
        let charged = db
            .brain_outbound_count_on_role(&day, "reviewer")
            .expect("outbound count");

        // 前提：叫不起來的那兩次，稽核紀錄真的記了、真的算進今天的額度。
        assert!(
            charged > 0,
            "前提變了：叫不起 CLI 不再寫 outbound 紀錄，這條測試要重寫"
        );

        // 真的被扣掉的，和這一輪自己說被扣掉的，要是同一個數字。
        assert_eq!(
            result.budget_used, charged,
            "outbound 紀錄扣了 {charged} 次，這一輪卻說用掉 {} 次",
            result.budget_used
        );

        let stats = RecheckStats {
            runs: Some(1),
            candidates: Some(1),
            rechecks: Some(0),
            last_skip: None,
        };
        let shown = format_review_result(&result, &stats);

        // 使用者看得到的那一句：不可以說額度一次都沒動。
        assert!(
            !shown.contains(&format!(
                "預算還剩 {}/{}",
                result.budget_limit, result.budget_limit
            )),
            "今天已經被扣掉 {charged} 次了，畫面卻說一次都沒用：{shown}"
        );
    }

    /// 叫不起 CLI 的一輪，不是「雙 pass 的分歧」，更不是「其中一個 pass 沒有
    /// 可用的 JSON」——兩個 pass 都沒起來。
    #[test]
    fn a_round_whose_cli_never_started_is_not_a_dual_pass_divergence() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(
            &mut db,
            "sister-no-such-brain-binary-9d3f".into(),
            vec![],
            ts,
        );
        assert!(result.skip.is_none(), "{:?}", result.skip);
        assert!(result.calls_used > 0);

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().expect("divergences"),
            &db.latest_reviewer_refusals().expect("refusals"),
            &db.latest_reviewer_notes().expect("notes"),
            &db.entity_memory().expect("entities"),
        );
        assert!(
            shown.contains("兩個 pass 都叫不起 CLI"),
            "這一輪叫不起 CLI，畫面要講出來：{shown}"
        );
        assert!(
            !shown.contains("的分歧"),
            "沒有兩份答案，不可以被端成「雙 pass 的分歧」：{shown}"
        );
        assert!(
            !shown.contains("其中一個 pass 沒有可用的 JSON"),
            "兩個 pass 都沒起來，不可以說「其中一個」沒有 JSON：{shown}"
        );
    }

    /// 叫不起 CLI 的一輪，不是「Reviewer 跑過，但目前沒有活著的實體」。
    /// 那句話把「問不到」講成「辨識到 0 個」。
    #[test]
    fn a_round_whose_cli_never_started_is_not_identified_zero_entities() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(
            &mut db,
            "sister-no-such-brain-binary-9d3f".into(),
            vec![],
            ts,
        );
        assert!(result.skip.is_none(), "{:?}", result.skip);
        assert!(result.calls_used > 0);

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().expect("divergences"),
            &db.latest_reviewer_refusals().expect("refusals"),
            &db.latest_reviewer_notes().expect("notes"),
            &db.entity_memory().expect("entities"),
        );
        // 「審閱」出現在分歧那一塊的標題裡，守不住實體那一段。
        assert!(
            shown.contains("審閱") || shown.contains("拒絕"),
            "這一塊還是要有話講：{shown}"
        );
        assert!(
            shown.contains("Reviewer 試過問模型，但沒拿到答案；目前不是『辨識到 0 個』"),
            "實體那一塊還是要有話講（擋『靠刪掉整段來過』）：{shown}"
        );
        assert!(
            !shown.contains("沒有活著的實體"),
            "一個模型行程都沒起來的一輪，不可以被端成「辨識到 0 個」：{shown}"
        );
    }

    #[test]
    fn pass_excerpt_keeps_what_the_cli_said_when_spawn_failed() {
        let spawn = SpawnOutcome {
            payload_chars_written: 0,
            duration_ms: 1,
            stdout: "Error: not logged in".into(),
            stderr: String::new(),
            timed_out: false,
            spawn_error: Some("寫入 CLI stdin 失敗：Broken pipe".into()),
            exit_code: Some(1),
            process_start: ProcessStart::Started,
        };
        let shown = pass_excerpt(&spawn);
        assert!(
            shown.contains("寫入 CLI stdin 失敗"),
            "我們自己的錯誤不見了：{shown}"
        );
        assert!(
            shown.contains("Error: not logged in"),
            "CLI 說的話被丟掉了：{shown}"
        );
    }

    #[test]
    fn outbound_error_explains_both_exit_code_shapes_without_rust_debug_syntax() {
        let nonzero = unanswered_cli_error(Some(7));
        let interrupted = unanswered_cli_error(None);

        assert_eq!(nonzero, "CLI 以退出碼 7 結束，沒有回答");
        assert_eq!(interrupted, "CLI 被中止了，沒有回答");
        for shown in [&nonzero, &interrupted] {
            assert!(
                !shown.contains("Some(") && !shown.contains("None"),
                "使用者看得到的外送錯誤不可以漏出 Rust Option debug 形狀：{shown}"
            );
        }
    }

    #[test]
    fn a_started_cli_that_exits_is_not_called_could_not_start() {
        let failed = SpawnOutcome {
            payload_chars_written: 0,
            duration_ms: 1,
            stdout: "not logged in".into(),
            stderr: String::new(),
            timed_out: false,
            spawn_error: Some("寫入 CLI stdin 失敗：Broken pipe".into()),
            exit_code: Some(1),
            process_start: ProcessStart::Started,
        };
        let reason = no_usable_json_reason((None, None), &failed, &failed);
        assert!(
            !reason.contains("叫不起 CLI"),
            "行程起來了還說叫不起：{reason}"
        );
        assert!(
            reason.contains("寫入 CLI stdin 失敗"),
            "真正的原因不見了：{reason}"
        );
    }

    #[test]
    fn only_named_common_failures_are_coalesced_for_two_passes() {
        let failed = SpawnOutcome {
            payload_chars_written: 0,
            duration_ms: 1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            spawn_error: Some("寫入 CLI stdin 失敗：Broken pipe".into()),
            exit_code: Some(1),
            process_start: ProcessStart::Started,
        };
        let reason = no_usable_json_reason((None, None), &failed, &failed);
        assert!(
            reason.contains("pass A") && reason.contains("pass B"),
            "任意相同錯誤不能被改寫成『兩個 pass 都…』：{reason}"
        );

        let never_started = SpawnOutcome {
            process_start: ProcessStart::NeverStarted,
            spawn_error: Some("叫不起 CLI".into()),
            ..failed
        };
        assert_eq!(
            no_usable_json_reason((None, None), &never_started, &never_started),
            format!("兩個 pass 都{COULD_NOT_START_CLI}")
        );
    }

    /// `migrate_016` 的白名單靠這個開頭把舊 `detail` 裡的說明搬到 `notes`。
    /// 直接呼叫產品的拒絕路徑，確認每一種拒絕都不會撞上它。
    #[test]
    fn missing_segment_note_head_does_not_match_a_real_refusal() {
        let mut db = Db::open_in_memory().expect("db");
        let url_id = db.test_insert_fact(1, "url", "example.com").expect("url");
        let title_id = db
            .test_insert_fact(2, "window_title", "Editor")
            .expect("title");
        let url = db.fact_by_id(url_id).expect("query").expect("url row");
        let title = db.fact_by_id(title_id).expect("query").expect("title row");
        let mut swapped = url.clone();
        swapped.raw = "https://original.example/".into();
        let cases = [
            (Vec::new(), NextStepRef { fact: 999 }),
            (Vec::new(), NextStepRef { fact: url_id }),
            (vec![swapped], NextStepRef { fact: url_id }),
            (vec![url], NextStepRef { fact: url_id }),
            (vec![title], NextStepRef { fact: title_id }),
        ];
        for (listed, next) in cases {
            let ResolvedNextStep::Refused(reason) =
                resolve_allowed_next_step(&db, &listed, Some(&next)).expect("resolve")
            else {
                panic!("這個 case 應該被拒絕")
            };
            assert!(
                !reason.starts_with(MISSING_SEGMENT_NOTE_HEAD),
                "拒絕句對上了搬 notes 的白名單：{reason}"
            );
            assert!(
                reason.starts_with("拒絕 allowed_next_step："),
                "拒絕句改開頭了，白名單那一側也要重看：{reason}"
            );
        }
        assert!(
            !MISSING_SEGMENT_NOTE_HEAD.starts_with("拒絕 allowed_next_step："),
            "兩個開頭對上了"
        );
    }

    /// R1（鏡頭 A 第 1 條 ＝ 鏡頭 B G2，兩個獨立鏡頭撞到同一條）
    ///
    /// `GotNoAnswer` 那一臂以前印「最近一次審閱（輪次 #N）」，而 notes 那一塊
    /// 也印「最近一次審閱（輪次 #M）」。兩塊取的是不同的列：
    /// refusals 要 `calls_used > 0`，notes（`latest_reviewer_run_copy`）不要。
    /// 所以同一個畫面上，同一個標籤會配上兩個不同輪次。現在這條守的是
    /// `GotNoAnswer` 不要退回那個和 notes 共用標籤的版本。
    #[test]
    fn ted_r1_gotnoanswer_shares_a_label_with_the_notes_block() {
        let shown = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &ReviewerRefusals::GotNoAnswer { run_id: 5 },
            &ReviewerNotes::Some {
                run_id: 6,
                lines: vec!["這張卡片指的那一段查不到".into()],
            },
            &EntityMemory::NeverReviewed,
        );
        assert_no_shared_label_across_rounds(&shown);
    }

    /// R2（鏡頭 B G3）
    ///
    /// `entity_memory` 的 `got_answer` 以前掃整張表，歷史上任何一輪問到過答案，
    /// 就會替最新那輪一個行程都沒起來的結果背書。現在它有 ORDER BY / LIMIT，
    /// 和隔壁兩個鏡頭取同一列；這條守的是不要退回掃整張表。
    ///
    /// 特別保留較早一輪真的問到答案的前提，避免只測一顆從頭到尾都失敗的資料庫。
    #[test]
    fn ted_r2_a_dead_cli_round_is_not_zero_entities_even_after_an_earlier_good_round() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );

        // 更早的一輪：真的問到了答案。唯一和上面那條測試的差別就是這一段。
        let earlier = ts - 60_000;
        let day = crate::brain::local_day_key(earlier).expect("day key");
        db.insert_reviewer_run(&crate::db::ReviewerRunInsert {
            ts: earlier,
            day_key: &day,
            kind: "interval",
            skip_reason: None,
            candidate_count: Some(1),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 2,
            budget_used: 2,
            budget_limit: 40,
            detail: "",
            notes: "",
            answers_got: Some(2),
        })
        .expect("earlier run");

        let result = run_diverge(
            &mut db,
            "sister-no-such-brain-binary-9d3f".into(),
            vec![],
            ts,
        );
        assert!(result.skip.is_none(), "這一輪要真的跑過：{:?}", result.skip);

        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().expect("divergences"),
            &db.latest_reviewer_refusals().expect("refusals"),
            &db.latest_reviewer_notes().expect("notes"),
            &db.entity_memory().expect("entities"),
        );

        assert!(
            !shown.contains("Reviewer 跑過，但目前沒有活著的實體"),
            "這一輪一個模型行程都沒起來，畫面卻說跑過而且沒有實體：{shown}"
        );
    }

    /// schema 17 之前的舊列，`answers_got` 是 NULL＝**沒量過**，不是量過是零。
    ///
    /// `migrate_017` 刻意不加 `DEFAULT 0`（`db.rs:641-648` 的註解），就是為了讓
    /// 這兩種 0 分得開。這條釘的是讀取端有沒有照那份契約走。
    ///
    /// 我在看修法之前寫的。**這一條在 round 9 上是綠的**，它是迴歸網不是 bug
    /// 報告：把 `db.rs:4970` 寫成 `answers_got.unwrap_or(0) == 0` 會讓它紅。
    /// 我實測過那個突變今天是**全綠**的（`cargo test --workspace`，19 個
    /// test result 行），而同一個突變打在孿生的 `latest_dual_pass_divergences`
    /// （`db.rs:5029`）會紅 2 條——同一份 NULL 契約三個讀取端，只有兩個有牙齒。
    ///
    /// 使用者端的後果：任何 schema 17 之前建的資料庫，升級後最新那一列的
    /// `answers_got` 都是 NULL。讀錯的話整份拒絕清單會消失，畫面改口說
    /// 「沒拿到模型的答案」——那正是這個分支在修的病，從 migration 走回來。
    #[test]
    fn ted_an_old_row_without_answers_got_must_keep_showing_its_refusals() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let day = crate::brain::local_day_key(ts).expect("day key");
        db.insert_reviewer_run(&ReviewerRunInsert {
            ts,
            day_key: &day,
            kind: "interval",
            skip_reason: None,
            candidate_count: Some(1),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 2,
            budget_used: 2,
            budget_limit: 40,
            detail: "把某某某的信回掉",
            notes: "",
            // 這一欄是 NULL：這一列是升級前寫的，那時候還沒有人在量。
            answers_got: None,
        })
        .expect("old row");

        let refusals = db.latest_reviewer_refusals().expect("refusals");
        assert!(
            matches!(refusals, ReviewerRefusals::Some { .. }),
            "NULL 是「沒量過」，不是「量過是零」；舊列的拒絕清單不可以消失：{refusals:?}"
        );

        let shown = format_reviewer_visibility(
            &DualPassDivergences::NeverRan,
            &refusals,
            &ReviewerNotes::NeverRan,
            &EntityMemory::NeverReviewed,
        );
        assert!(
            shown.contains("把某某某的信回掉"),
            "升級之後那一步的拒絕就看不見了：{shown}"
        );
        assert!(
            !shown.contains("沒拿到模型的答案"),
            "把 NULL 當成 0 了——這一列從來沒被量過：{shown}"
        );
    }

    /// **「分歧」數的是「兩份答案對不上」，不是「一份答案都沒有」。**
    ///
    /// `reviewer.rs:438-439` 自己就是這樣定義那個數字的。可是 `pair => {}`
    /// 那一臂（一份可用 JSON 都沒有的時候）照樣 `divergences += 1`，於是同一次
    /// `sister review` 的輸出上會**背靠背**印出這兩句：
    ///
    /// - 摘要（`reviewer.rs:171`）：「……分歧 1 筆……」
    /// - 四行底下（`reviewer.rs:239`）：「最近一次雙 pass（審閱輪次 #N）
    ///   **沒有拿到可比較的兩份答案**」
    ///
    /// 兩句話都在同一個畫面上（`ops.rs:1753` 緊接著 `ops.rs:1766`）。
    /// 一顆叫不起 CLI 的機器，會被告知它有 1 筆「分歧」。
    ///
    /// 這條測試不規定怎麼改（可以是不加、也可以是摘要那行分開講），
    /// 只要求：**沒問到答案的那一輪，不要在摘要上被說成有分歧。**
    #[test]
    fn ted_r11_a_round_with_no_answers_has_no_divergence_in_the_summary() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_250_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(
            &mut db,
            "sister-no-such-brain-binary-9d3f".into(),
            vec![],
            ts,
        );
        assert!(result.skip.is_none(), "{:?}", result.skip);
        assert!(result.calls_used > 0, "這一輪要真的試著叫過 CLI");

        assert_eq!(
            result.divergences, 0,
            "一份答案都沒拿到，卻在摘要上記了 {} 筆「分歧」。\
             而同一個畫面四行底下寫的是「沒有拿到可比較的兩份答案」——\
             同一次輸出的兩段話互相打臉。",
            result.divergences
        );

        // ⚠ 這一半和上面一樣重要：**不可以拿「分歧 0」換來第二種 0。**
        //
        // 這個 repo 最貴的那個病就是「一個 0 有兩個意思」。把這一輪從
        // 「分歧 1」改成「分歧 0」之後，摘要那個數字現在同時代表
        // 「兩份答案都拿到了而且一致」和「一份答案都沒拿到」——除非畫面上
        // 另外有人把後者講出來。
        //
        // 所以這裡直接要求那句話存在，而且**不准用 `||` 讓別的句子替它補票**
        // （既有那條 `a_round_whose_cli_never_started_is_not_a_dual_pass_divergence`
        // 就是這樣被架空的：右邊那個 disjunct 是標題，沒答案就無條件印，
        // 於是左邊永遠不必為真）。
        let shown = format_reviewer_visibility(
            &db.latest_dual_pass_divergences().expect("divergences"),
            &db.latest_reviewer_refusals().expect("refusals"),
            &db.latest_reviewer_notes().expect("notes"),
            &db.entity_memory().expect("entities"),
        );
        assert!(
            shown.contains("叫不起"),
            "分歧記成 0 之後，畫面就必須自己講出「這一輪根本沒問到」，\
             否則「分歧 0」同時代表「都同意」和「沒問到」——\
             又一個有兩個意思的 0。畫面現在是：\n{shown}"
        );
    }

    /// `parse_usable_pass` 的那道 guard 沒有任何測試。
    ///
    /// 實測：把 `reviewer.rs` 的 `if !spawn.completed_the_ask() {` 改成
    /// `if false {`（＝整個停用這一輪的核心修法），`cargo test --workspace`
    /// **19 個 binary 全綠**。這一版最重要的那一行，刪掉沒有人會發現。
    ///
    /// 這條就是那個突變的守衛：一個「沒問到」的 spawn，即使 stdout 是一份
    /// 完全合法的 JSON，也不可以被 parse 成一張卡。
    #[test]
    fn ted_r11_an_unanswered_spawn_yields_no_card_even_with_valid_json() {
        let json = r#"{"commitments":[],"model_confidence":0.9}"#;

        // 三種「沒問到」，每一種都配一份看起來完全可用的 stdout。
        let cases = [
            (
                "提示沒送完",
                SpawnOutcome {
                    payload_chars_written: 3,
                    duration_ms: 1,
                    stdout: json.into(),
                    stderr: String::new(),
                    timed_out: false,
                    spawn_error: Some("寫入 CLI stdin 失敗：Broken pipe".into()),
                    exit_code: Some(1),
                    process_start: ProcessStart::Started,
                },
            ),
            (
                "逾時",
                SpawnOutcome {
                    payload_chars_written: 700,
                    duration_ms: 120_000,
                    stdout: json.into(),
                    stderr: String::new(),
                    timed_out: true,
                    spawn_error: None,
                    exit_code: None,
                    process_start: ProcessStart::Started,
                },
            ),
            (
                "非零退出（沒登入）",
                SpawnOutcome {
                    payload_chars_written: 700,
                    duration_ms: 5,
                    stdout: json.into(),
                    stderr: String::new(),
                    timed_out: false,
                    spawn_error: None,
                    exit_code: Some(1),
                    process_start: ProcessStart::Started,
                },
            ),
        ];

        for (name, spawn) in cases {
            assert!(
                parse_pass(&spawn.stdout).is_some(),
                "夾具壞了：{name} 這一格的 stdout 應該是**看得懂**的 JSON，\
                 否則這條測試證不出 guard 有沒有作用"
            );
            assert!(
                parse_usable_pass(&spawn).is_none(),
                "{name}：這一輪沒有問到答案，stdout 裡那份 JSON 不是這一題的\
                 回覆，不可以拿去寫承諾"
            );
        }
    }
    /// **那句話要從真的出口出來，不是從 helper 出來。**
    ///
    /// round 12 加了 `unanswered_cli_error()`，也加了一條測試——可是那條測試
    /// 直接呼叫 helper 比字串。我實測過：把 `log_outbound()` 裡那句
    /// `.or_else(|| Some(unanswered_cli_error(spawn.exit_code)))` **整個刪掉**
    /// （＝這一輪對使用者的改善完全沒接上去），`cargo test --workspace`
    /// **19 個 binary 全綠**。
    ///
    /// 這條走真的路：真的開一支 `sh`、真的讓它非零退出、真的讓 reviewer 把
    /// 那一列寫進 `brain_outbound`，然後讀**存進去的那個欄位**——也就是
    /// `sister brain log` 原文印給使用者看的那一格（`ops.rs` 那個
    /// `if let Some(err) = &row.error { println!("    {err}") }`）。
    #[test]
    fn ted_r12_the_outbound_row_a_user_reads_says_it_in_words() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_260_000_000;
        let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00");
        let core = db.chapters_for_range(ts, ts + 400_000).expect("segs")[0].core_started_at;
        write_l2(
            &mut db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );

        // 讀完 stdin 再非零退出：不讀的話寫入端會撞上斷管，那是**另一個**
        // 成因（`spawn_error` 會被設起來），這條就測不到退出碼那一臂了。
        let result = run_diverge(
            &mut db,
            "sh".into(),
            vec!["-c".into(), "cat >/dev/null; exit 7".into()],
            ts,
        );
        assert!(result.skip.is_none(), "{:?}", result.skip);
        assert!(result.calls_used > 0, "這一輪要真的叫過 CLI");

        let rows = db.list_brain_outbound(10).expect("outbound log");
        assert!(!rows.is_empty(), "外送紀錄是空的，這一輪根本沒送");
        for row in &rows {
            let shown = row
                .error
                .as_deref()
                .unwrap_or_else(|| panic!("這一列沒有錯誤訊息，使用者只會看到一個結局：{row:?}"));
            assert!(
                !shown.contains("Some(") && !shown.contains("None"),
                "使用者在 `sister brain log` 看到的是 Rust 的 Option debug：{shown}"
            );
            assert!(
                shown.contains('7'),
                "沒有回答的原因是退出碼 7，那個數字要講出來：{shown}"
            );
        }
    }
    fn run_diverge(db: &mut Db, command: String, args: Vec<String>, ts: Millis) -> ReviewResult {
        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = ReviewInput {
            db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            kind: ReviewKind::Interval,
            force: true,
            now: ts + 500_000,
        };
        run(&mut input).expect("run")
    }

    fn refuse_a_step_at(db: &mut Db, tmp: &Tmp, ts: Millis, url: &str) -> i64 {
        let sentinel = tmp.0.join(format!("spawned-{ts}"));
        let (_sid, fid) = seed(db, ts, "LINE：五點去接她 17:00");
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        let core = segs[0].core_started_at;
        db.conn
            .execute("DELETE FROM segment", [])
            .expect("drop the segment this card points at");
        let fact_ts = ts + crate::segment::TIME_CAP_MS + 30_000;
        let fact_id = db.test_insert_fact(fact_ts, "url", url).expect("fact");
        let json = format!(
            r#"{{"commitments":[{{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:{fid}"],"allowed_next_step":{{"fact":{fact_id}}}}}]}}"#
        );
        let (command, args) = fake_cli(&tmp.0, &json, &sentinel);
        write_l2(
            db,
            core,
            fid,
            "在看接人的訊息",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let result = run_diverge(db, command, args, ts);
        assert_eq!(result.refused_next_steps, 1, "要真的拒絕過：{url}");
        fact_id
    }

    #[test]
    fn a_commitment_only_one_pass_saw_names_its_l2() {
        let tmp = Tmp::new("diverge-solo");
        let both = r#"{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:1"]}"#;
        let only_a = r#"{"text":"順路買牛奶","stands":true,"kind":"promise","due_hint":"18:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:1"]}"#;
        let (mut db, l2, command, args, ts) = diverge_fixture(
            &tmp,
            &format!(r#"{{"commitments":[{both},{only_a}]}}"#),
            &format!(r#"{{"commitments":[{both}]}}"#),
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.divergences, 1, "只有一邊看到的那筆該記成分歧");
        let rows = db.list_reviewer_divergences(10).expect("div");
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].reason.contains("只有其中一個 pass"),
            "{}",
            rows[0].reason
        );
        assert!(
            rows[0].subject.contains(&format!("l2:{l2}")),
            "沒說是哪一張 L2：{}",
            rows[0].subject
        );
        assert!(
            rows[0].subject.contains("順路買牛奶"),
            "沒說是哪一筆：{}",
            rows[0].subject
        );
    }

    #[test]
    fn an_unparseable_pass_names_its_l2() {
        // 一支 CLI 壞掉／被限流／回了一句人話，是最常見的分歧來源，
        // 而它是**整張卡**沒有結果，所以 subject 只能靠 L2 ref 認人。
        let tmp = Tmp::new("diverge-garbage");
        let (mut db, l2, command, args, ts) = diverge_fixture(
            &tmp,
            r#"{"commitments":[{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:1"]}]}"#,
            "抱歉，我今天沒辦法回答這個問題。",
        );
        let result = run_diverge(&mut db, command, args, ts);
        assert_eq!(result.divergences, 1);
        assert_eq!(result.wrote_commitments, 0, "只有一個 pass 讀得懂還敢寫 L3");
        let rows = db.list_reviewer_divergences(10).expect("div");
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].reason.contains("沒有可用的 JSON"),
            "{}",
            rows[0].reason
        );
        assert!(
            rows[0].subject.contains(&format!("l2:{l2}")),
            "沒說是哪一張 L2：{}",
            rows[0].subject
        );
    }

    #[test]
    fn lifecycle_paths_do_not_invent_user_interaction() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_400_000_000;
        let (_sid, fid) = seed(&mut db, ts, "帳單 NT$80 已完成");
        let l2 = write_l2(
            &mut db,
            ts,
            fid,
            "在看帳單",
            r#"[{"text":"繳帳單","source":"畫面","due_hint":"17:00"}]"#,
        );
        let now = ts + 10_000;
        let id = db
            .insert_commitment(
                l3_write(),
                &CommitmentInsert {
                    text: "繳帳單",
                    kind: "todo",
                    born_from: l2,
                    evidence_json: format!(r#"["frame:{fid}"]"#),
                    agreed_evidence_json: Some(format!(r#"["frame:{fid}"]"#)),
                    people_json: "[]".into(),
                    due_hint: Some("17:00"),
                    due_source: Some("explicit"),
                    due_at: Some(now - ARCHIVE_GRACE_MS - 1_000),
                    status: "open",
                    confidence: 0.7,
                    allowed_next_step: None,
                    allowed_next_step_fact: None,
                    last_evidence_seen_at: Some(now),
                    kill_note: None,
                    now: now - ARCHIVE_GRACE_MS - 10_000,
                },
            )
            .expect("open");
        assert_eq!(db.commitment_by_id(id).unwrap().unwrap().status, "open");

        kill_commitment(&mut db, id, "弄好了", now).unwrap();
        assert_eq!(db.commitment_by_id(id).unwrap().unwrap().status, "dead");

        let id2 = db
            .insert_commitment(
                l3_write(),
                &CommitmentInsert {
                    text: "回信",
                    kind: "followup",
                    born_from: l2,
                    evidence_json: format!(r#"["frame:{fid}"]"#),
                    agreed_evidence_json: Some(format!(r#"["frame:{fid}"]"#)),
                    people_json: "[]".into(),
                    due_hint: None,
                    due_source: None,
                    due_at: None,
                    status: "open",
                    confidence: 0.5,
                    allowed_next_step: None,
                    allowed_next_step_fact: None,
                    last_evidence_seen_at: None,
                    kill_note: None,
                    now,
                },
            )
            .unwrap();
        snooze_commitment(&mut db, id2, now).unwrap();
        assert_eq!(db.commitment_by_id(id2).unwrap().unwrap().status, "snoozed");
        assert!(db.preference("snoozed_kind:followup").unwrap().is_some());

        let id3 = db
            .insert_commitment(
                l3_write(),
                &CommitmentInsert {
                    text: "過期的事",
                    kind: "reminder",
                    born_from: l2,
                    evidence_json: format!(r#"["frame:{fid}"]"#),
                    agreed_evidence_json: Some(format!(r#"["frame:{fid}"]"#)),
                    people_json: "[]".into(),
                    due_hint: Some("17:00"),
                    due_source: Some("inferred"),
                    due_at: Some(now - ARCHIVE_GRACE_MS - 5_000),
                    status: "open",
                    confidence: 0.4,
                    allowed_next_step: None,
                    allowed_next_step_fact: None,
                    last_evidence_seen_at: None,
                    kill_note: None,
                    now: now - ARCHIVE_GRACE_MS - 5_000,
                },
            )
            .unwrap();
        // 誕生後一次寫入都沒有 → 規則 3 走得到底。
        let overdue = db.commitment_by_id(id3).unwrap().unwrap();
        assert_eq!(overdue.updated_at, overdue.created_at);
        assert_eq!(archive_decision(&overdue, now), ArchiveDecision::Archive);

        // 同一張卡，只要**被寫過一次**（誰寫的分不出來），就退回不歸檔。
        // 這兩個 assert 是一組的：只留上面那一個，把 `updated_at != created_at`
        // 那道門拿掉也不會紅；只留下面那一個，整條規則關掉也不會紅。
        let touched = crate::db::CommitmentRow {
            updated_at: overdue.created_at + 1,
            ..overdue.clone()
        };
        assert!(matches!(
            archive_decision(&touched, now),
            ArchiveDecision::InteractionUnknown { .. }
        ));

        assert_eq!(archive_overdue(&mut db, now).unwrap(), 1);
        assert_eq!(
            db.commitment_by_id(id3).unwrap().unwrap().status,
            "archived"
        );

        let id4 = db
            .insert_commitment(
                l3_write(),
                &CommitmentInsert {
                    text: "螢幕上已完成",
                    kind: "todo",
                    born_from: l2,
                    evidence_json: format!(r#"["frame:{fid}"]"#),
                    agreed_evidence_json: Some(format!(r#"["frame:{fid}"]"#)),
                    people_json: "[]".into(),
                    due_hint: None,
                    due_source: None,
                    due_at: None,
                    status: "open",
                    confidence: 0.6,
                    allowed_next_step: None,
                    allowed_next_step_fact: None,
                    last_evidence_seen_at: None,
                    kill_note: None,
                    now,
                },
            )
            .unwrap();
        let consent = signed();
        let brain = BrainConfig::default();
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            kind: ReviewKind::Eod,
            force: true,
            now,
        };
        let n = mark_done_from_originals(&mut input).unwrap();
        assert!(n >= 1, "完成畫面沒把承諾標成 done");
        assert_eq!(db.commitment_by_id(id4).unwrap().unwrap().status, "done");
    }

    #[test]
    fn user_correction_is_not_overwritten_by_reviewer() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_500_000_000;
        let (_sid, fid) = seed(&mut db, ts, "在改測試");
        write_l2(&mut db, ts, fid, "她猜錯的活動", r#"[]"#);
        let id = correct_l2(&mut db, ts, "其實在寫文件").unwrap();
        let latest = db.latest_l2_for_segment(ts).unwrap().unwrap();
        assert_eq!(latest.id, id);
        assert_eq!(latest.author, L2Author::User);
        assert_eq!(latest.activity, "其實在寫文件");
        let err = db
            .insert_l2_card(&L2Insert {
                segment_core_start: ts,
                segment_ref: &format!("segment:{ts}"),
                activity: "想蓋掉",
                entities_json: "[]".into(),
                continues_json: None,
                commitments_json: "[]".into(),
                model_confidence: 0.9,
                evidence_json: format!(r#"["frame:{fid}"]"#),
                open_questions_json: "[]".into(),
                author: L2Author::Reviewer,
            })
            .unwrap_err();
        assert!(err.to_string().contains("使用者改過"));
        assert_eq!(
            db.latest_l2_for_segment(ts).unwrap().unwrap().activity,
            "其實在寫文件"
        );
    }

    #[test]
    fn forgetting_l0_tombstones_derived_l2_and_l3() {
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_600_000_000;
        let (_sid, fid) = seed(&mut db, ts, "王小明 五點去接她 17:00 NT$80");
        let l2 = write_l2(
            &mut db,
            ts,
            fid,
            "在看接人",
            r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
        );
        let cid = db
            .insert_commitment(
                l3_write(),
                &CommitmentInsert {
                    text: "五點去接她",
                    kind: "promise",
                    born_from: l2,
                    evidence_json: format!(r#"["frame:{fid}"]"#),
                    agreed_evidence_json: Some(format!(r#"["frame:{fid}"]"#)),
                    people_json: r#"["王小明"]"#.into(),
                    due_hint: Some("17:00"),
                    due_source: Some("explicit"),
                    due_at: Some(ts + 3_600_000),
                    status: "open",
                    confidence: 0.8,
                    allowed_next_step: None,
                    allowed_next_step_fact: Some(99),
                    last_evidence_seen_at: Some(ts),
                    kill_note: None,
                    now: ts,
                },
            )
            .unwrap();
        let eid = db
            .upsert_entity(l3_write(), "person", "王小明", &format!("l2:{l2}"), ts)
            .unwrap();
        db.insert_entity_mention(l3_write(), eid, &format!("l2:{l2}"), ts)
            .unwrap();
        db.insert_day_summary(
            l3_write(),
            &DaySummaryInsert {
                date: "2023-11-16",
                narrative: "在看接人",
                session_refs_json: format!(r#"["l2:{l2}","segment:{ts}"]"#),
                stats_json: "{}".into(),
                now: ts,
            },
        )
        .unwrap();

        db.forget(ts, ts + 400_000, None).expect("forget");

        assert!(db.l0_original(&EvidenceRef::Frame(fid)).unwrap().is_none());
        let l2_row = db.l2_by_id(l2).unwrap().unwrap();
        assert!(l2_row.tombstoned_at.is_some(), "L2 應該是墓碑不是消失");
        assert!(db.latest_l2_for_segment(ts).unwrap().is_none());
        let c = db.commitment_by_id(cid).unwrap().unwrap();
        assert!(
            c.tombstoned_at.is_some(),
            "commitment 應 tombstone，不是實刪"
        );
        let mentions = db.live_mentions_for(eid).unwrap();
        assert!(mentions.is_empty(), "提及應跟著死");
        let ents = db.live_entities().unwrap();
        assert!(
            ents.iter().all(|e| e.id != eid),
            "沒有 live mention 的 entity 應 tombstone，還活著的是 {:?}",
            ents
        );
        assert!(
            db.latest_day_summary("2023-11-16").unwrap().is_none(),
            "日摘要應 tombstone"
        );
        let still: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM l2_card", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still, 1, "tombstone 不是 DELETE，列還在");
        let still_c: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM commitments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(still_c, 1);
        assert_eq!(
            c.allowed_next_step_fact, None,
            "commitment 墓碑必須清掉動作目標的 fact id"
        );
    }

    #[test]
    fn due_source_explicit_needs_the_clock_on_the_original() {
        let on_screen = vec![crate::db::L0Original {
            r#ref: "frame:1".into(),
            kind: "frame",
            text: "會議 17:00 開始".into(),
        }];
        let guessed = vec![crate::db::L0Original {
            r#ref: "frame:2".into(),
            kind: "frame",
            text: "下午再看那件事".into(),
        }];
        assert_eq!(due_source_from_originals("17:00", &on_screen), "explicit");
        assert_eq!(due_source_from_originals("17:00", &guessed), "inferred");
    }

    #[test]
    fn cadence_skips_instead_of_polling() {
        let mut db = Db::open_in_memory().expect("db");
        let now = 1_700_700_000_000;
        db.insert_reviewer_run(&ReviewerRunInsert {
            ts: now,
            day_key: "2023-11-17",
            kind: "interval",
            skip_reason: None,
            candidate_count: Some(0),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 0,
            budget_used: 0,
            budget_limit: 40,
            detail: "",
            notes: "",
            answers_got: None,
        })
        .unwrap();
        let consent = signed();
        let brain = BrainConfig {
            command: "python3".into(),
            args: vec![],
            ..Default::default()
        };
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: now,
            to_ts: now + 1,
            kind: ReviewKind::Interval,
            force: false,
            now: now + 60_000,
        };
        let result = run(&mut input).expect("run");
        assert!(matches!(result.skip, Some(SkipReason::Cadence { .. })));
        assert!(!result.ran);
    }

    #[test]
    fn merge_does_not_vote_away_a_disagreement() {
        let a = ReviewCommitment {
            text: "五點去接她".into(),
            stands: true,
            kind: Some("promise".into()),
            due_hint: Some("17:00".into()),
            due_source: Some("explicit".into()),
            people: vec![],
            confidence: Some(0.9),
            evidence_refs: vec!["frame:1".into()],
            agreed_evidence_refs: vec![],
            allowed_next_step: None,
        };
        let b = ReviewCommitment {
            text: "五點去接她".into(),
            stands: true,
            kind: Some("promise".into()),
            due_hint: Some("18:00".into()),
            due_source: Some("explicit".into()),
            people: vec![],
            confidence: Some(0.9),
            evidence_refs: vec!["frame:1".into()],
            agreed_evidence_refs: vec![],
            allowed_next_step: None,
        };
        let err = merge_commitment_passes(&a, &b).unwrap_err();
        assert!(err.reason.contains("due_hint"));
    }

    fn merge_card(refs: &[&str]) -> ReviewCommitment {
        ReviewCommitment {
            text: "五點去接她".into(),
            stands: true,
            kind: Some("promise".into()),
            due_hint: Some("17:00".into()),
            due_source: Some("explicit".into()),
            people: vec![],
            confidence: Some(0.8),
            evidence_refs: refs.iter().map(|r| (*r).to_string()).collect(),
            agreed_evidence_refs: vec!["frame:injected".into()],
            allowed_next_step: None,
        }
    }

    #[test]
    fn merge_unions_evidence_and_intersects_agreed() {
        let merged = merge_commitment_passes(
            &merge_card(&["frame:1", "frame:2"]),
            &merge_card(&["frame:2", "frame:3"]),
        )
        .expect("same fields");
        assert_eq!(merged.evidence_refs, vec!["frame:1", "frame:2", "frame:3"]);
        assert_eq!(merged.agreed_evidence_refs, vec!["frame:2"]);
    }

    #[test]
    fn parse_pass_cannot_inject_agreed_evidence_refs() {
        let json = r#"{"commitments":[{"text":"五點去接她","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":[],"confidence":0.8,"evidence_refs":["frame:1"],"agreed_evidence_refs":["frame:999"]}]}"#;
        let parsed = parse_pass(json).expect("parse");
        assert_eq!(parsed.commitments[0].evidence_refs, ["frame:1"]);
        assert!(
            parsed.commitments[0].agreed_evidence_refs.is_empty(),
            "模型塞進來的 agreed_evidence_refs 必須被丟掉：{:?}",
            parsed.commitments[0].agreed_evidence_refs
        );
    }

    #[test]
    fn l3_write_is_only_constructible_here() {
        let _ = l3_write();
        let _unused = VERSION;
    }

    /// **叫不起來的那一輪，稽核紀錄要說零。**
    ///
    /// `spawn_cli` 在 spawn 失敗時是在寫 stdin **之前**就 return 的——那一路
    /// 一個位元組都沒有離開這台機器。alpha.77 之前這裡記的是整包 payload 的
    /// 字數，於是一台沒裝那支 CLI 的機器，`sister brain log` 上每一輪都寫著
    /// 送出去幾千個字。這個產品的賣點就是那句話，往「送出去了」的方向錯是
    /// 最糟的方向。
    ///
    /// 兩個方向一起釘：成功那一次必須 > 0，否則寫死一個 0 也會過（實測過，
    /// 只釘零的話突變 `chars_sent: 0` 是綠的）。
    #[test]
    fn a_review_that_never_spawned_records_zero_characters_sent() {
        fn outbound_chars(command: String, args: Vec<String>) -> Vec<i64> {
            let mut db = Db::open_in_memory().expect("db");
            let ts = 1_700_200_000_000;
            let (_sid, fid) = seed(&mut db, ts, "LINE：五點去接她 17:00 王小明");
            let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
            write_l2(
                &mut db,
                segs[0].core_started_at,
                fid,
                "在看接人的訊息",
                r#"[{"text":"五點去接她","source":"LINE","due_hint":"17:00"}]"#,
            );
            let consent = signed();
            let brain = BrainConfig {
                command,
                args,
                ..Default::default()
            };
            let mut input = ReviewInput {
                db: &mut db,
                consent: &consent,
                brain: &brain,
                from_ts: ts,
                to_ts: ts + 400_000,
                kind: ReviewKind::Interval,
                force: true,
                now: ts + 500_000,
            };
            let _ = run(&mut input).expect("run");
            db.list_brain_outbound(10)
                .expect("outbound")
                .into_iter()
                .filter(|row| row.role == "reviewer")
                .map(|row| row.chars_sent)
                .collect()
        }

        let never_spawned = outbound_chars(
            "sister-no-such-brain-cli-6f2a".to_string(),
            vec!["--json".to_string()],
        );
        assert!(!never_spawned.is_empty(), "那兩列 reviewer 紀錄要在");
        assert!(
            never_spawned.iter().all(|&chars| chars == 0),
            "叫不起來就是一個字都沒送出去，不可以記成整包：{never_spawned:?}"
        );

        let tmp = Tmp::new("outbound-chars-sent");
        let sentinel = tmp.0.join("spawned");
        let (command, args) = fake_cli(&tmp.0, r#"{"commitments":[]}"#, &sentinel);
        let really_sent = outbound_chars(command, args);
        assert!(!really_sent.is_empty(), "那兩列 reviewer 紀錄要在");
        assert!(
            really_sent.iter().all(|&chars| chars > 0),
            "真的送出去的那一輪不可以記成零，否則寫死一個 0 也會過：{really_sent:?}"
        );
    }

    /// 日終幾乎永遠在跨日之後才跑。摘要的 `date` 是被盤點的那天，不是跑的那天。
    #[test]
    fn eod_after_midnight_summarizes_yesterday_not_today() {
        let tmp = Tmp::new("eod-cross-midnight");
        let sentinel = tmp.0.join("spawned");
        let (command, args) = fake_cli(&tmp.0, r#"{"commitments":[]}"#, &sentinel);

        let Some(sunday_evening) = local_ms(2026, 8, 23, 22, 0, 0) else {
            return;
        };
        let Some(monday_morning) = local_ms(2026, 8, 24, 10, 0, 0) else {
            return;
        };
        let Some(monday_afternoon) = local_ms(2026, 8, 24, 16, 0, 0) else {
            return;
        };
        let Some(tuesday_wee) = local_ms(2026, 8, 25, 0, 0, 30) else {
            return;
        };
        let Some(tuesday_eod) = local_ms(2026, 8, 25, 0, 1, 0) else {
            return;
        };

        let mut db = Db::open_in_memory().expect("db");
        let (_sid, fid_sun) = seed(&mut db, sunday_evening, "星期天晚上還在");
        write_l2(&mut db, sunday_evening, fid_sun, "星期天晚上還在", "[]");
        let (_sid, fid_am) = seed(&mut db, monday_morning, "星期一早上改測試");
        write_l2(&mut db, monday_morning, fid_am, "星期一早上改測試", "[]");
        let (_sid, fid_pm) = seed(&mut db, monday_afternoon, "星期一下午讀 SPEC");
        write_l2(&mut db, monday_afternoon, fid_pm, "星期一下午讀 SPEC", "[]");
        let (_sid, fid_tue) = seed(&mut db, tuesday_wee, "星期二凌晨的卡片");
        write_l2(&mut db, tuesday_wee, fid_tue, "星期二凌晨的卡片", "[]");

        let consent = signed();
        let brain = BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = ReviewInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: tuesday_eod.saturating_sub(36 * 3_600_000),
            to_ts: tuesday_eod,
            kind: ReviewKind::Eod,
            force: false,
            now: tuesday_eod,
        };
        let result = run(&mut input).expect("eod");
        assert!(result.skip.is_none(), "日終被跳過：{:?}", result.skip);
        assert!(result.ran);

        let monday = brain::local_day_key(monday_morning).expect("monday");
        let tuesday = brain::local_day_key(tuesday_eod).expect("tuesday");
        assert_eq!(monday, "2026-08-24");
        assert_eq!(tuesday, "2026-08-25");

        match db.day_summary_glance(&monday).unwrap() {
            DaySummaryGlance::Live { date, clauses, .. } => {
                assert_eq!(date, monday);
                let texts: Vec<&str> = clauses.iter().map(|c| c.text.as_str()).collect();
                assert_eq!(texts, ["星期一早上改測試", "星期一下午讀 SPEC"]);
            }
            other => panic!("星期一該是 Live：{other:?}"),
        }
        let tuesday_glance = db.day_summary_glance(&tuesday).unwrap();
        assert!(
            !matches!(tuesday_glance, DaySummaryGlance::Live { .. }),
            "星期二不該掛著星期一的摘要：{tuesday_glance:?}"
        );
        assert!(
            matches!(tuesday_glance, DaySummaryGlance::NeverRan { .. }),
            "星期二自己還沒被盤點：{tuesday_glance:?}"
        );
        assert_eq!(
            db.last_reviewer_eod_day().unwrap().as_deref(),
            Some(tuesday.as_str()),
            "run stamp 仍是跑的那天，eod_due 靠這個擋同一天再跑"
        );
    }
}
