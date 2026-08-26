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
    self, CommitmentCandidate, Entity, EvidenceRef, OutboundOutcome, SpawnOutcome, spawn_cli,
};
use crate::config::BrainConfig;
use crate::consent::Consent;
use crate::db::{
    CommitmentInsert, DaySummaryInsert, Db, DivergenceInsert, DualPassDivergences, EntityMemory,
    L2Author, L2CardRow, L2Insert, OutboundInsert, RecheckInsert, RecheckStats, ReviewerRunInsert,
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
        match self {
            SkipReason::NoConsent => concat!(
                "還沒簽第二張同意書（上雲解讀）。審閱層一次都不會呼叫那支 CLI。\n",
                "要簽字：sister consent --grant cloud-reading"
            )
            .to_string(),
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
                "這段期間沒有還沒審過的 L2 假設。\n\
                 （同意書已簽、CLI 已設定、預算還剩 {remaining} 次。）"
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
    let mut out = String::new();
    if let Some(skip) = &r.skip {
        if r.ran {
            out.push_str(&skip.message());
            out.push('\n');
        } else {
            out.push_str("真的跑的話會停在這裡：\n");
            out.push_str(&skip.message());
            out.push('\n');
        }
    } else {
        out.push_str(&format!(
            "審閱結束：回查 {}／{}，寫入 {} 筆承諾，分歧 {} 筆，L2 修訂 {} 張。\n",
            r.rechecks, r.candidates, r.wrote_commitments, r.divergences, r.l2_revisions
        ));
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
    entities: &EntityMemory,
) -> String {
    // 分歧是**警報**，不是統計行的續行。前後各空一行，讓它自己是一段。
    let mut out = String::from("\n");
    match divergences {
        DualPassDivergences::NeverRan => {
            out.push_str("雙 pass 還沒跑過；目前沒有可比較的兩份答案。\n");
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
    match entities {
        EntityMemory::NeverReviewed => {
            out.push_str("實體記憶還沒跑過 Reviewer；目前不是『辨識到 0 個』。\n");
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
            detail: &reason.message(),
        })?;
        return Ok(ReviewResult {
            skip: Some(reason),
            ran: true,
            rechecks: 0,
            candidates: 0,
            wrote_commitments: 0,
            divergences: 0,
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
    let mut calls = 0u32;
    let mut l2_revisions = 0u32;
    let mut budget_left = remaining;

    let mut recheck_rows: Vec<RecheckInsertOwned> = Vec::new();
    let mut divergence_rows: Vec<(String, String, String, String)> = Vec::new();

    for card in &cards {
        if card.author == L2Author::User {
            continue;
        }
        let facts = input.db.facts_in_range(
            card.segment_core_start,
            card.segment_core_start.saturating_add(3_600_000),
        )?;
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

        let prompt_a = dual_pass_prompt('A', card, &originals, &keep_commits);
        let prompt_b = dual_pass_prompt('B', card, &originals, &keep_commits);
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
        log_outbound(
            input.db, &run_day, &command, &args, card, &prompt_a, &spawn_a,
        )?;
        log_outbound(
            input.db, &run_day, &command, &args, card, &prompt_b, &spawn_b,
        )?;

        let parsed_a = parse_pass(&spawn_a.stdout);
        let parsed_b = parse_pass(&spawn_b.stdout);
        match (parsed_a, parsed_b) {
            (Some(a), Some(b)) => {
                let map_a = by_text(&a.commitments);
                let map_b = by_text(&b.commitments);
                let keys: BTreeSet<_> = map_a.keys().chain(map_b.keys()).cloned().collect();
                for key in keys {
                    match (map_a.get(&key), map_b.get(&key)) {
                        (Some(ca), Some(cb)) => match merge_commitment_passes(ca, cb) {
                            Ok(merged) if merged.stands => {
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
                                let kind = merged.kind.as_deref().unwrap_or("followup");
                                input.db.insert_commitment(
                                    l3_write(),
                                    &CommitmentInsert {
                                        text: &merged.text,
                                        kind,
                                        born_from: card.id,
                                        evidence_json,
                                        people_json,
                                        due_hint: merged.due_hint.as_deref(),
                                        due_source: Some(source),
                                        due_at,
                                        status: "open",
                                        confidence: merged
                                            .confidence
                                            .unwrap_or(card.model_confidence)
                                            .min(card.model_confidence),
                                        allowed_next_step: None,
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
            _ => {
                divergences += 1;
                divergence_rows.push((
                    format!("l2:{} / {}", card.id, card.activity),
                    spawn_a.stdout.chars().take(400).collect(),
                    spawn_b.stdout.chars().take(400).collect(),
                    "其中一個 pass 沒有可用的 JSON".into(),
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
        let summarized_day = brain::previous_local_day_key(input.now)
            .context("算不出被盤點的那一天，不敢寫日摘要")?;
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
        detail: "",
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
    commits: &[CommitmentCandidate],
) -> String {
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
    {"text":"...","stands":true,"kind":"promise","due_hint":"17:00","due_source":"explicit","people":["..."],"confidence":0.7,"evidence_refs":["frame:1"]}
  ]
}
"#,
    );
    s.push_str("due_source 只能是 explicit（螢幕上寫了時間）或 inferred（你從上下文猜的）。\n");
    s.push_str("kind 只能是 promise / todo / followup / reminder。\n\n");
    s.push_str("L2 假設（可推翻，不是原件）：\n");
    s.push_str(&format!("- activity: {}\n", card.activity));
    s.push_str("承諾候選：\n");
    for c in commits {
        s.push_str(&format!(
            "- {}（來源 {}） due={:?}\n",
            c.text, c.source, c.due_hint
        ));
    }
    s.push_str("\n—— L0 原件（真的去讀的，不是 L2 再抄一次）——\n");
    for o in originals {
        s.push_str(&format!("[{}] {}\n", o.r#ref, o.text));
    }
    s
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
        duration_ms: 0,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        spawn_error: Some(msg.into()),
        exit_code: None,
    }
}

fn log_outbound(
    db: &mut Db,
    day: &str,
    command: &str,
    args: &[String],
    card: &L2CardRow,
    payload: &str,
    spawn: &SpawnOutcome,
) -> Result<()> {
    let (outcome, error) = if spawn.spawn_error.is_some() {
        (OutboundOutcome::SpawnFailed, spawn.spawn_error.clone())
    } else if spawn.timed_out {
        (OutboundOutcome::Timeout, Some("逾時".into()))
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
        chars_sent: payload.chars().count() as i64,
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
        let text = format_reviewer_visibility(&divergence, &entities);
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
            DualPassDivergences::Agreed { run_id: 7 },
            DualPassDivergences::Diverged {
                run_id: 7,
                rows: vec![row],
            },
        ]
        .map(|s| format_reviewer_visibility(&s, &EntityMemory::NeverReviewed));
        let e = [
            EntityMemory::NeverReviewed,
            EntityMemory::Empty,
            EntityMemory::Present(vec![entity]),
        ]
        .map(|s| format_reviewer_visibility(&DualPassDivergences::NeverRan, &s));

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

        // 「三句話不一樣」還不夠。「沒有分歧」是一句**比較過**才講得出口的話，
        // 只有 Agreed 有資格講；NeverRan 沒有兩份答案可比，Diverged 剛好相反。
        // 少了這三條，把 NeverRan 改成印「沒有分歧。」照樣三句不同、照樣全綠。
        assert!(d[0].contains("還沒跑過"), "{}", d[0]);
        assert!(!d[0].contains("沒有分歧"), "還沒比就說沒有分歧：{}", d[0]);
        assert!(d[1].contains("沒有分歧"), "{}", d[1]);
        assert!(!d[2].contains("沒有分歧"), "有分歧卻說沒有：{}", d[2]);

        assert!(e[0].contains("還沒跑過"), "{}", e[0]);
        assert!(
            !e[0].contains("沒有活著的實體"),
            "沒跑過卻說辨識到 0 個：{}",
            e[0]
        );
        assert!(e[1].contains("沒有活著的實體"), "{}", e[1]);
        assert!(!e[2].contains("沒有活著的實體"), "有實體卻說沒有：{}", e[2]);
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
                    people_json: "[]".into(),
                    due_hint: Some("17:00"),
                    due_source: Some("explicit"),
                    due_at: Some(now - ARCHIVE_GRACE_MS - 1_000),
                    status: "open",
                    confidence: 0.7,
                    allowed_next_step: None,
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
                    people_json: "[]".into(),
                    due_hint: None,
                    due_source: None,
                    due_at: None,
                    status: "open",
                    confidence: 0.5,
                    allowed_next_step: None,
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
                    people_json: "[]".into(),
                    due_hint: Some("17:00"),
                    due_source: Some("inferred"),
                    due_at: Some(now - ARCHIVE_GRACE_MS - 5_000),
                    status: "open",
                    confidence: 0.4,
                    allowed_next_step: None,
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
                    people_json: "[]".into(),
                    due_hint: None,
                    due_source: None,
                    due_at: None,
                    status: "open",
                    confidence: 0.6,
                    allowed_next_step: None,
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
                    people_json: r#"["王小明"]"#.into(),
                    due_hint: Some("17:00"),
                    due_source: Some("explicit"),
                    due_at: Some(ts + 3_600_000),
                    status: "open",
                    confidence: 0.8,
                    allowed_next_step: None,
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
        };
        let err = merge_commitment_passes(&a, &b).unwrap_err();
        assert!(err.reason.contains("due_hint"));
    }

    #[test]
    fn l3_write_is_only_constructible_here() {
        let _ = l3_write();
        let _unused = VERSION;
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
