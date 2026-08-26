//! L2 假設層：把螢幕上的字交給使用者設定的 CLI，收回一張 JSON 卡片。
//!
//! 出境路徑是 `std::process::Command`。沒有 HTTP client、沒有本機推論引擎。
//! spawn 要 [`crate::consent::CloudAllowed`]，只有同意書 2 鑄得出來——
//! 沒簽就送，編不過。
//!
//! **送出去的是原文，不去敏。** 記憶長期活在本機資料庫裡，而代號是每次呼叫
//! 重編的，跨段對不起來：`<PERSON_1>` 在這一段和下一段不是同一個人。
//! 承諾表和 entities 要的正是「王小明」這三個字能對得起來。同意書 2 第 3 版
//! 講的就是這件事，他按的是那句話。

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::BrainConfig;
use crate::consent::{CloudAllowed, Consent};
use crate::db::{Db, FactRow, L2CardRow, L2Insert, OutboundInsert};
use crate::model::Millis;
use crate::model::SearchHit;
use crate::segment::{CutKind, LARGE_CLIPBOARD_BYTES, Segment};

/// 一次 spawn 等多久。CLI 掛住不能把解釋層卡住。
pub const SPAWN_TIMEOUT: Duration = Duration::from_secs(120);
/// 整份 prompt 的位元組上限。超過就截斷，並在外送紀錄記 `truncated`。
pub const MAX_PROMPT_BYTES: usize = 24 * 1024;
/// OCR 摘錄最多帶幾段。不是設定項：超過就截，說得出來。
pub const MAX_OCR_SNIPPETS: usize = 40;

/// 為什麼這一趟沒送出去。每一種印出來的字都不一樣。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    NoConsent,
    NoCommand,
    BudgetExhausted { used: u32, limit: u32 },
    NothingWorthInterpreting { remaining: u32 },
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::NoConsent => "no_consent",
            SkipReason::NoCommand => "no_command",
            SkipReason::BudgetExhausted { .. } => "budget",
            SkipReason::NothingWorthInterpreting { .. } => "nothing_worth",
        }
    }

    pub fn message(&self) -> String {
        match self {
            SkipReason::NoConsent => concat!(
                "還沒簽第二張同意書（上雲解讀）。解釋層一次都不會呼叫那支 CLI。\n",
                "要看她準備送出什麼：sister interpret --dry-run\n",
                "要簽字：sister consent --grant cloud-reading"
            )
            .to_string(),
            SkipReason::NoCommand => concat!(
                "還沒設定 [brain] command。一次都不會呼叫。\n",
                "（不是今天沒有東西可解釋——她根本沒有一支 CLI 可以叫。）\n",
                "在設定檔加上例如：\n",
                "  [brain]\n",
                "  command = \"claude\"\n",
                "  args = [\"-p\"]"
            )
            .to_string(),
            SkipReason::BudgetExhausted { used, limit } => {
                format!("今天的解釋預算已用完（{used}/{limit}）。超過即靜默降級，只累積 L0/L1。")
            }
            SkipReason::NothingWorthInterpreting { remaining } => format!(
                "這段期間沒有「值得理解」的已關閉段落。\n\
                 （同意書已簽、CLI 已設定、預算還剩 {remaining} 次。）"
            ),
        }
    }
}

/// 一次 spawn 的結果。不含送出去的原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnOutcome {
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub spawn_error: Option<String>,
    pub exit_code: Option<i32>,
}

impl SpawnOutcome {
    pub fn started(&self) -> bool {
        self.spawn_error.is_none()
    }
}

/// 外送紀錄的結局。寫進資料庫的是這些字，不是 stdout。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundOutcome {
    Success,
    SpawnFailed,
    Timeout,
    BadJson,
}

impl OutboundOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            OutboundOutcome::Success => "success",
            OutboundOutcome::SpawnFailed => "spawn_failed",
            OutboundOutcome::Timeout => "timeout",
            OutboundOutcome::BadJson => "bad_json",
        }
    }

    pub fn from_str_kind(s: &str) -> Option<Self> {
        match s {
            "success" => Some(Self::Success),
            "spawn_failed" => Some(Self::SpawnFailed),
            "timeout" => Some(Self::Timeout),
            "bad_json" => Some(Self::BadJson),
            _ => None,
        }
    }
}

/// 真正把字送出程序的那一扇門。
///
/// 第一個參數的型別是 [`CloudAllowed`]：只有 [`Consent::cloud_permit`] 鑄得
/// 出來，所以「沒檢查同意書就送出去」是編不過的，不是靠每個呼叫端自己記得。
pub fn spawn_cli(
    permit: CloudAllowed,
    payload: &str,
    command: &str,
    args: &[String],
) -> SpawnOutcome {
    let _gate = permit;
    let started = Instant::now();
    let mut child = match Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return SpawnOutcome {
                duration_ms: started.elapsed().as_millis() as u64,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                spawn_error: Some(format!("叫不起 `{command}`：{e}")),
                exit_code: None,
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }

    let mut stdout_pipe = match child.stdout.take() {
        Some(p) => p,
        None => {
            let _ = child.kill();
            return SpawnOutcome {
                duration_ms: started.elapsed().as_millis() as u64,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                spawn_error: Some("stdout 管線沒開成".into()),
                exit_code: None,
            };
        }
    };
    let stderr_pipe = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = stderr_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() >= SPAWN_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                return SpawnOutcome {
                    duration_ms: started.elapsed().as_millis() as u64,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: false,
                    spawn_error: Some(format!("等 CLI 結束失敗：{e}")),
                    exit_code: None,
                };
            }
        }
    };

    let stdout = String::from_utf8_lossy(&stdout_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_thread.join().unwrap_or_default()).into_owned();
    let exit_code = child.wait().ok().and_then(|s| s.code());

    SpawnOutcome {
        duration_ms: started.elapsed().as_millis() as u64,
        stdout,
        stderr,
        timed_out,
        spawn_error: None,
        exit_code,
    }
}

/// 模型回的 JSON。缺欄、型別錯、範圍外 → 整張丟掉，不填預設值。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelCard {
    pub segment_ref: String,
    pub activity: String,
    pub entities: Vec<Entity>,
    #[serde(default)]
    pub continues: Option<Continues>,
    #[serde(default)]
    pub commitment_candidates: Vec<CommitmentCandidate>,
    pub confidence: f64,
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Continues {
    pub segment_ref: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitmentCandidate {
    pub text: String,
    pub source: String,
    #[serde(default)]
    pub due_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceRef {
    Frame(i64),
    Fact(i64),
}

impl EvidenceRef {
    pub fn as_str(&self) -> String {
        match self {
            EvidenceRef::Frame(id) => format!("frame:{id}"),
            EvidenceRef::Fact(id) => format!("fact:{id}"),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let (kind, rest) = s.split_once(':')?;
        let id: i64 = rest.parse().ok()?;
        if id <= 0 {
            return None;
        }
        match kind {
            "frame" => Some(EvidenceRef::Frame(id)),
            "fact" => Some(EvidenceRef::Fact(id)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCard {
    pub segment_ref: String,
    pub activity: String,
    pub entities: Vec<Entity>,
    pub continues: Option<Continues>,
    pub commitment_candidates: Vec<CommitmentCandidate>,
    /// 模型自己講的數字，不是量出來的。
    pub model_confidence: f64,
    pub evidence_refs: Vec<EvidenceRef>,
    pub open_questions: Vec<String>,
}

/// 從 CLI stdout 抽出一張卡片。壞掉就是 `None`，不編一張看起來正常的。
pub fn parse_card(stdout: &str, expected_segment_ref: &str) -> Result<ParsedCard, String> {
    let value =
        extract_json_object(stdout).ok_or_else(|| "stdout 裡找不到 JSON 物件".to_string())?;
    let card: ModelCard =
        serde_json::from_value(value).map_err(|e| format!("JSON 對不上契約：{e}"))?;

    if card.segment_ref.trim().is_empty() {
        return Err("segment_ref 是空的".into());
    }
    if card.segment_ref != expected_segment_ref {
        return Err(format!(
            "segment_ref 對不上（模型說 {}，這一段是 {expected_segment_ref}）",
            card.segment_ref
        ));
    }
    if card.activity.trim().is_empty() {
        return Err("activity 是空的".into());
    }
    if !(0.0..=1.0).contains(&card.confidence) {
        return Err(format!("confidence {} 不在 0..=1", card.confidence));
    }
    if let Some(c) = &card.continues {
        if c.segment_ref.trim().is_empty() {
            return Err("continues.segment_ref 是空的".into());
        }
        if !(0.0..=1.0).contains(&c.confidence) {
            return Err(format!("continues.confidence {} 不在 0..=1", c.confidence));
        }
    }
    for e in &card.entities {
        if e.kind.trim().is_empty() || e.name.trim().is_empty() {
            return Err("entities 裡有空的 type 或 name".into());
        }
    }
    for c in &card.commitment_candidates {
        if c.text.trim().is_empty() || c.source.trim().is_empty() {
            return Err("commitment_candidates 裡有空的 text 或 source".into());
        }
    }
    if card.evidence_refs.is_empty() {
        return Err("evidence_refs 是空的——沒有根據的卡片不寫進去".into());
    }
    let mut refs = Vec::new();
    for raw in &card.evidence_refs {
        let Some(r) = EvidenceRef::parse(raw) else {
            return Err(format!("看不懂的 evidence_ref：{raw}"));
        };
        refs.push(r);
    }

    Ok(ParsedCard {
        segment_ref: card.segment_ref,
        activity: card.activity,
        entities: card.entities,
        continues: card.continues,
        commitment_candidates: card.commitment_candidates,
        model_confidence: card.confidence,
        evidence_refs: refs,
        open_questions: card.open_questions,
    })
}

fn extract_json_object(stdout: &str) -> Option<serde_json::Value> {
    let trimmed = stdout.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
        && v.is_object()
    {
        return Some(v);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }
    let slice = &trimmed[start..=end];
    let v: serde_json::Value = serde_json::from_str(slice).ok()?;
    v.is_object().then_some(v)
}

pub fn segment_ref(core_started_at: Millis) -> String {
    format!("segment:{core_started_at}")
}

/// 一段關閉的段落值不值得花一次預算。
pub fn worth_interpreting(seg: &Segment, facts: &[FactRow], large_clip: bool, stuck: bool) -> bool {
    if seg.cut_kinds.iter().any(|k| {
        matches!(
            k,
            CutKind::IdleResume
                | CutKind::ClipboardPaste
                | CutKind::AppChange
                | CutKind::HostChange
        )
    }) {
        return true;
    }
    if stuck || large_clip {
        return true;
    }
    facts.iter().any(|f| f.kind == "error_code")
}

pub fn local_day_key(ts: Millis) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ts).map(|d| {
        d.with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string()
    })
}

/// `--dry-run` 印出來的那一份。一個字都不送。
#[derive(Debug, Clone)]
pub struct DryRun {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub consent: bool,
    pub budget_used: u32,
    pub budget_limit: u32,
    pub jobs: Vec<PreparedJob>,
    pub skip: Option<SkipReason>,
}

#[derive(Debug, Clone)]
pub struct PreparedJob {
    pub segment_ref: String,
    pub core_started_at: Millis,
    pub core_ended_at: Millis,
    pub app: Option<String>,
    pub title: Option<String>,
    pub payload: String,
    /// 證據那一半有沒有被 `MAX_PROMPT_BYTES` 截掉。
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct InterpretResult {
    pub skip: Option<SkipReason>,
    pub ran: Vec<RanJob>,
}

#[derive(Debug, Clone)]
pub struct RanJob {
    pub segment_ref: String,
    pub outcome: OutboundOutcome,
    pub duration_ms: u64,
    pub card: Option<ParsedCard>,
    pub error: Option<String>,
}

pub struct InterpretInput<'a> {
    pub db: &'a mut Db,
    pub consent: &'a Consent,
    pub brain: &'a BrainConfig,
    pub from_ts: Millis,
    pub to_ts: Millis,
    pub limit: usize,
    /// 指定某一段的 core_started_at。有的話跳過「值不值得」那一關。
    pub only_core_start: Option<Millis>,
}

pub fn prepare(input: &mut InterpretInput<'_>) -> Result<DryRun> {
    let configured = input.brain.cli();
    let used = today_used(input.db)?;
    let remaining = input.brain.daily_budget.saturating_sub(used);

    let jobs = collect_jobs(input)?;

    let skip = if configured.is_none() {
        Some(SkipReason::NoCommand)
    } else if input.consent.cloud_permit().is_none() {
        Some(SkipReason::NoConsent)
    } else if remaining == 0 && !jobs.is_empty() {
        Some(SkipReason::BudgetExhausted {
            used,
            limit: input.brain.daily_budget,
        })
    } else if jobs.is_empty() {
        Some(SkipReason::NothingWorthInterpreting { remaining })
    } else {
        None
    };

    let (command, args) = match configured {
        Some((c, a)) => (Some(c.to_string()), a.to_vec()),
        None => (None, Vec::new()),
    };

    Ok(DryRun {
        command,
        args,
        consent: input.consent.cloud_permit().is_some(),
        budget_used: used,
        budget_limit: input.brain.daily_budget,
        jobs,
        skip,
    })
}

/// 真的跑。沒有 [`CloudAllowed`] 就一次都不 spawn。
pub fn run(input: &mut InterpretInput<'_>) -> Result<InterpretResult> {
    let Some(permit) = input.consent.cloud_permit() else {
        return Ok(InterpretResult {
            skip: Some(SkipReason::NoConsent),
            ran: Vec::new(),
        });
    };
    let Some((command, args)) = input.brain.cli() else {
        return Ok(InterpretResult {
            skip: Some(SkipReason::NoCommand),
            ran: Vec::new(),
        });
    };
    let command = command.to_string();
    let args = args.to_vec();

    let used = today_used(input.db)?;
    if used >= input.brain.daily_budget {
        record_skip(
            input.db,
            SkipReason::BudgetExhausted {
                used,
                limit: input.brain.daily_budget,
            },
        )?;
        return Ok(InterpretResult {
            skip: Some(SkipReason::BudgetExhausted {
                used,
                limit: input.brain.daily_budget,
            }),
            ran: Vec::new(),
        });
    }

    let remaining = input.brain.daily_budget.saturating_sub(used);
    let slots = input.brain.concurrency_slots() as usize;
    let take = remaining.min(slots as u32).min(input.limit as u32) as usize;

    let mut prepared = collect_jobs(input)?;
    if prepared.is_empty() {
        return Ok(InterpretResult {
            skip: Some(SkipReason::NothingWorthInterpreting { remaining }),
            ran: Vec::new(),
        });
    }
    prepared.truncate(take);

    let mut ran = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = prepared
            .iter()
            .map(|job| {
                let command = command.as_str();
                let args = args.as_slice();
                let payload = &job.payload;
                scope.spawn(move || spawn_cli(permit, payload, command, args))
            })
            .collect();
        for (job, handle) in prepared.iter().zip(handles) {
            let outcome = handle.join().unwrap_or_else(|_| SpawnOutcome {
                duration_ms: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
                spawn_error: Some("工作執行緒炸了".into()),
                exit_code: None,
            });
            ran.push((job.clone(), outcome));
        }
    });

    let day = local_day_key(crate::now_ms()).context("算不出今天的日期，不敢送")?;
    let mut results = Vec::new();
    for (job, spawn) in ran {
        let (kind, card, error) = classify(&job, &spawn, input.db)?;
        input.db.insert_brain_outbound(&OutboundInsert {
            ts: crate::now_ms(),
            day_key: &day,
            command: &command,
            args: &args,
            segment_core_start: Some(job.core_started_at),
            chars_sent: job.payload.chars().count() as i64,
            truncated: job.truncated,
            outcome: kind.as_str(),
            duration_ms: spawn.duration_ms as i64,
            error: error.as_deref(),
            role: "interpreter",
        })?;
        if let Some(card) = &card {
            let evidence: Vec<String> = card.evidence_refs.iter().map(|r| r.as_str()).collect();
            input.db.insert_l2_card(&L2Insert {
                segment_core_start: job.core_started_at,
                segment_ref: &job.segment_ref,
                activity: &card.activity,
                entities_json: serde_json::to_string(&card.entities)?,
                continues_json: card
                    .continues
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                commitments_json: serde_json::to_string(&card.commitment_candidates)?,
                model_confidence: card.model_confidence,
                evidence_json: serde_json::to_string(&evidence)?,
                open_questions_json: serde_json::to_string(&card.open_questions)?,
                author: crate::db::L2Author::Interpreter,
            })?;
        }
        results.push(RanJob {
            segment_ref: job.segment_ref,
            outcome: kind,
            duration_ms: spawn.duration_ms,
            card,
            error,
        });
    }

    Ok(InterpretResult {
        skip: None,
        ran: results,
    })
}

fn classify(
    job: &PreparedJob,
    spawn: &SpawnOutcome,
    db: &Db,
) -> Result<(OutboundOutcome, Option<ParsedCard>, Option<String>)> {
    if let Some(e) = &spawn.spawn_error {
        return Ok((OutboundOutcome::SpawnFailed, None, Some(e.clone())));
    }
    if spawn.timed_out {
        return Ok((
            OutboundOutcome::Timeout,
            None,
            Some(format!("等了 {} 秒還沒結束", SPAWN_TIMEOUT.as_secs())),
        ));
    }
    match parse_card(&spawn.stdout, &job.segment_ref) {
        Ok(mut card) => {
            card.evidence_refs.retain(|r| match r {
                EvidenceRef::Frame(id) => db.frame_exists(*id).unwrap_or(false),
                EvidenceRef::Fact(id) => db.fact_exists(*id).unwrap_or(false),
            });
            if card.evidence_refs.is_empty() {
                return Ok((
                    OutboundOutcome::BadJson,
                    None,
                    Some("evidence_refs 沒有任何一筆指得回本機的 frame／fact".into()),
                ));
            }
            Ok((OutboundOutcome::Success, Some(card), None))
        }
        Err(e) => {
            let extra = if spawn.exit_code.unwrap_or(0) != 0 {
                format!("{e}（CLI 退出碼 {}）", spawn.exit_code.unwrap_or(-1))
            } else {
                e
            };
            Ok((OutboundOutcome::BadJson, None, Some(extra)))
        }
    }
}

fn today_used(db: &Db) -> Result<u32> {
    let day = local_day_key(crate::now_ms()).context("算不出今天的日期")?;
    db.brain_outbound_count_on(&day)
}

fn record_skip(db: &mut Db, reason: SkipReason) -> Result<()> {
    db.insert_brain_skip(crate::now_ms(), reason.as_str(), None, &reason.message())?;
    Ok(())
}

fn collect_jobs(input: &mut InterpretInput<'_>) -> Result<Vec<PreparedJob>> {
    let segs = input.db.chapters_for_range(input.from_ts, input.to_ts)?;
    let stuck = input.db.stuck_in_range(input.from_ts, input.to_ts)?;
    let mut jobs = Vec::new();
    let cap = input
        .limit
        .max(1)
        .min(input.brain.concurrency_slots() as usize * 4);

    for seg in segs.into_iter().rev() {
        if input
            .only_core_start
            .is_some_and(|only| seg.core_started_at != only)
        {
            continue;
        }
        if input
            .db
            .latest_l2_for_segment(seg.core_started_at)?
            .is_some()
        {
            continue;
        }
        let facts = input
            .db
            .facts_in_range(seg.core_started_at, seg.core_ended_at)?;
        let clips = input
            .db
            .clipboard_in_range(seg.core_started_at, seg.core_ended_at)?;
        let large_clip = clips.iter().any(|c| c.byte_len >= LARGE_CLIPBOARD_BYTES);
        let is_stuck = stuck
            .iter()
            .any(|s| s.started_at < seg.core_ended_at && s.ended_at > seg.core_started_at);
        if input.only_core_start.is_none()
            && !worth_interpreting(&seg, &facts, large_clip, is_stuck)
        {
            continue;
        }
        let prev = input.db.latest_l2_before(seg.core_started_at)?;
        let ocr =
            input
                .db
                .chunks_in_range(seg.core_started_at, seg.core_ended_at, MAX_OCR_SNIPPETS)?;
        let (header, evidence) = build_prompt(&seg, &facts, &ocr, prev.as_ref());
        // 只截證據那一半。說明是契約，截掉了模型就不知道要回什麼形狀。
        let (evidence, truncated) = crate::redact::truncate_utf8(&evidence, MAX_PROMPT_BYTES);
        jobs.push(PreparedJob {
            segment_ref: segment_ref(seg.core_started_at),
            core_started_at: seg.core_started_at,
            core_ended_at: seg.core_ended_at,
            app: seg.app.clone(),
            title: seg.title.clone(),
            payload: format!("{header}{evidence}"),
            truncated,
        });
        if jobs.len() >= cap {
            break;
        }
    }
    jobs.reverse();
    Ok(jobs)
}

fn build_prompt(
    seg: &Segment,
    facts: &[FactRow],
    ocr: &[SearchHit],
    prev: Option<&L2CardRow>,
) -> (String, String) {
    let mut header = String::new();
    header.push_str(
        "你是一個本機記憶的解釋層。根據下面這段證據，產出一張 JSON 卡片。\n\
         這是假設，不是事實。不確定就降低 confidence，把問題放進 open_questions。\n\
         禁止把猜測寫成確定的事。只輸出一個 JSON 物件，不要 markdown、不要前後解說。\n\n",
    );
    header.push_str("契約：\n");
    header.push_str(
        r#"{
  "segment_ref": "segment:<core_started_at>",
  "activity": "一句話描述他在做什麼",
  "entities": [{"type":"project","name":"..."}],
  "continues": {"segment_ref":"...","confidence":0.7} 或 null,
  "commitment_candidates": [{"text":"...","source":"...","due_hint":"17:00"}],
  "confidence": 0.6,
  "evidence_refs": ["frame:123","fact:45"],
  "open_questions": ["..."]
}
"#,
    );
    header.push_str("\nevidence_refs 只能引用下面列出的 frame: 與 fact:。\n");
    match prev {
        Some(p) => {
            header.push_str(
                "可推翻的他人假設（僅一筆，不是事實，可以忽略或推翻）：\n- segment_ref: segment:",
            );
            header.push_str(&p.segment_core_start.to_string());
            header.push_str("\n  confidence（模型自己說的）: ");
            header.push_str(&p.model_confidence.to_string());
            header.push_str("\n\n");
        }
        None => {
            header.push_str("可推翻的他人假設：沒有。這是這一帶的第一張卡片。\n\n");
        }
    }
    header.push_str("本段 segment_ref：segment:");
    header.push_str(&seg.core_started_at.to_string());
    header.push_str("\n時間：");
    header.push_str(&seg.core_started_at.to_string());
    header.push_str(" – ");
    header.push_str(&seg.core_ended_at.to_string());
    header.push_str("（epoch 毫秒）\n");
    if !seg.cut_kinds.is_empty() {
        header.push_str("打開這一段的切刀：");
        for (i, kind) in seg.cut_kinds.iter().enumerate() {
            if i > 0 {
                header.push('、');
            }
            header.push_str(kind.as_str());
        }
        header.push('\n');
    }
    header.push_str("\n—— 以下是證據 ——\n");

    let mut evidence = String::new();
    if let Some(p) = prev {
        evidence.push_str(&format!("上一張假設 activity：{}\n", p.activity));
    }
    if let Some(app) = &seg.app {
        evidence.push_str(&format!("app：{app}\n"));
    }
    if let Some(title) = &seg.title {
        evidence.push_str(&format!("視窗標題：{title}\n"));
    }
    if let Some(host) = &seg.host {
        evidence.push_str(&format!("host：{host}\n"));
    }
    evidence.push_str("本機 L1 事實（程式抄的，不是猜的）：\n");
    if facts.is_empty() {
        evidence.push_str("（這一段沒抽出 typed fact）\n");
    } else {
        for f in facts {
            evidence.push_str(&format!(
                "- fact:{} {} raw={:?} at={}\n",
                f.id, f.kind, f.raw, f.ts
            ));
        }
    }
    evidence.push_str("\nOCR 摘錄：\n");
    if ocr.is_empty() {
        evidence.push_str("（這一段沒有留下文字）\n");
    } else {
        for hit in ocr {
            match hit.frame_id {
                Some(id) => evidence.push_str(&format!("- frame:{id} {}\n", hit.text)),
                None => evidence.push_str(&format!("- (無畫面) {}\n", hit.text)),
            }
        }
    }
    (header, evidence)
}

/// `sister interpret --dry-run` 的人話。
pub fn format_dry_run(report: &DryRun) -> String {
    let mut out = String::new();
    out.push_str("── 不會送出去（--dry-run）──\n\n");
    match &report.command {
        Some(c) => {
            let args = if report.args.is_empty() {
                String::new()
            } else {
                format!(" {}", report.args.join(" "))
            };
            out.push_str(&format!("命令：{c}{args}\n"));
        }
        None => out.push_str("命令：（還沒設定 [brain] command）\n"),
    }
    out.push_str(&format!(
        "同意書 2：{}\n",
        if report.consent {
            "已簽"
        } else {
            "沒簽——真的跑的話一次都不會呼叫"
        }
    ));
    let remaining = report.budget_limit.saturating_sub(report.budget_used);
    out.push_str(&format!(
        "今日預算：{}/{}，還剩 {} 次\n",
        report.budget_used, report.budget_limit, remaining
    ));
    if let Some(skip) = &report.skip {
        match skip {
            SkipReason::NothingWorthInterpreting { .. } => {
                out.push('\n');
                out.push_str(&skip.message());
                out.push('\n');
                return out;
            }
            SkipReason::NoCommand | SkipReason::NoConsent | SkipReason::BudgetExhausted { .. } => {
                out.push('\n');
                out.push_str("真的跑的話會停在這裡：\n");
                out.push_str(&skip.message());
                out.push('\n');
            }
        }
    }
    if report.jobs.is_empty() {
        return out;
    }
    for (i, job) in report.jobs.iter().enumerate() {
        out.push('\n');
        out.push_str(&format!("── 第 {} 段 {} ──\n", i + 1, job.segment_ref));
        let where_ = [job.app.as_deref(), job.title.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        if !where_.is_empty() {
            out.push_str(&format!("段落：{where_}\n"));
        }
        out.push_str(&format!(
            "截斷：{}（{} 字）\n",
            if job.truncated { "是" } else { "否" },
            job.payload.chars().count()
        ));
        out.push_str("\n──── 送出的全文（原文，沒有遮任何東西）────\n");
        out.push_str(&job.payload);
        out.push_str("\n──── 完 ────\n");
    }
    out
}

/// 給時間軸用的一張假設。
#[derive(Debug, Clone, Serialize)]
pub struct L2View {
    pub id: i64,
    pub segment_ref: String,
    pub activity: String,
    /// 模型自己講的，或審閱／使用者改過的。
    pub model_confidence: f64,
    pub confidence_source: &'static str,
    pub author: &'static str,
    pub version: i32,
    /// 審閱層後來改過。原版還在版本鏈裡。
    pub revised: bool,
    /// 使用者當場改過，下一輪 recompute 不會蓋掉。
    pub user_corrected: bool,
    /// 若這是後來的版本，上一版的 activity。沒有就是沒有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_activity: Option<String>,
    pub entities: Vec<Entity>,
    pub evidence: Vec<L2EvidenceView>,
    pub open_questions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct L2EvidenceView {
    pub kind: &'static str,
    pub id: i64,
    pub label: String,
}

pub fn view_from_row(row: &L2CardRow) -> L2View {
    view_from_row_with_previous(row, None)
}

pub fn view_from_row_with_previous(row: &L2CardRow, previous: Option<&L2CardRow>) -> L2View {
    let entities: Vec<Entity> = serde_json::from_str(&row.entities_json).unwrap_or_default();
    let refs: Vec<String> = serde_json::from_str(&row.evidence_json).unwrap_or_default();
    let questions: Vec<String> = serde_json::from_str(&row.open_questions_json).unwrap_or_default();
    L2View {
        id: row.id,
        segment_ref: row.segment_ref.clone(),
        activity: row.activity.clone(),
        model_confidence: row.model_confidence,
        confidence_source: row.author.confidence_source(),
        author: row.author.as_str(),
        version: row.version,
        revised: row.author == crate::db::L2Author::Reviewer,
        user_corrected: row.author == crate::db::L2Author::User,
        previous_activity: previous.map(|p| p.activity.clone()),
        entities,
        evidence: refs
            .iter()
            .filter_map(|s| EvidenceRef::parse(s))
            .map(|r| match r {
                EvidenceRef::Frame(id) => L2EvidenceView {
                    kind: "frame",
                    id,
                    label: format!("畫面 #{id}"),
                },
                EvidenceRef::Fact(id) => L2EvidenceView {
                    kind: "fact",
                    id,
                    label: format!("本機事實 #{id}"),
                },
            })
            .collect(),
        open_questions: questions,
    }
}

/// `data_dir` 只是給外送紀錄的除錯檔用；目前不寫原文。
pub fn debug_dir(_data_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::{Sheet, VERSION};
    use crate::db::Db;

    #[test]
    fn parse_rejects_missing_fields_instead_of_inventing_them() {
        let err = parse_card("{}", "segment:1").unwrap_err();
        assert!(err.contains("契約") || err.contains("missing"), "{err}");
    }

    #[test]
    fn parse_rejects_confidence_outside_unit_interval() {
        let raw = r#"{
            "segment_ref":"segment:1","activity":"x",
            "entities":[],"confidence":1.5,
            "evidence_refs":["frame:1"],"open_questions":[]
        }"#;
        let err = parse_card(raw, "segment:1").unwrap_err();
        assert!(err.contains("confidence"), "{err}");
    }

    #[test]
    fn parse_rejects_empty_evidence() {
        let raw = r#"{
            "segment_ref":"segment:1","activity":"x",
            "entities":[],"confidence":0.5,
            "evidence_refs":[],"open_questions":[]
        }"#;
        let err = parse_card(raw, "segment:1").unwrap_err();
        assert!(err.contains("evidence"), "{err}");
    }

    #[test]
    fn parse_accepts_a_well_formed_card() {
        let raw = r#"{
            "segment_ref":"segment:1",
            "activity":"在 Cloudflare dashboard 設定 DNS 記錄",
            "entities":[{"type":"project","name":"dns"}],
            "continues":null,
            "commitment_candidates":[],
            "confidence":0.6,
            "evidence_refs":["frame:12","fact:3"],
            "open_questions":["未看到儲存成功"]
        }"#;
        let card = parse_card(raw, "segment:1").expect("ok");
        assert_eq!(card.model_confidence, 0.6);
        assert_eq!(card.evidence_refs.len(), 2);
    }

    #[test]
    fn unsigned_consent_cannot_mint_a_permit() {
        let c = Consent::default();
        assert!(c.cloud_permit().is_none());
        let mut signed = Consent::default();
        signed.grant(Sheet::CloudReading, 1);
        assert!(signed.cloud_permit().is_some());
        signed.version = VERSION + 1;
        assert!(
            signed.cloud_permit().is_none(),
            "舊條文的簽名不能拿來送東西出去"
        );
    }

    #[test]
    fn spawn_requires_a_permit() {
        let mut c = Consent::default();
        c.grant(Sheet::CloudReading, 1);
        let permit = c.cloud_permit().expect("signed");
        let payload = "hello NT$80".to_string();

        let dir = std::env::temp_dir().join(format!("sister-fake-cli-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = dir.join("fake.py");
        std::fs::write(
            &script,
            "import sys\nsys.stdin.read()\nprint('{\"segment_ref\":\"segment:1\",\"activity\":\"x\",\"entities\":[],\"confidence\":0.5,\"evidence_refs\":[\"frame:1\"],\"open_questions\":[]}')\n",
        )
        .expect("write");
        let out = spawn_cli(
            permit,
            &payload,
            "python3",
            &[script.to_string_lossy().into_owned()],
        );
        assert!(out.spawn_error.is_none(), "{:?}", out.spawn_error);
        assert!(!out.timed_out);
        assert!(out.stdout.contains("segment_ref"), "{}", out.stdout);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn seed(db: &mut Db, ts: Millis) -> i64 {
        use crate::model::{FocusEvent, FocusKind, FocusSnapshot, FrameCapture, OcrBlock};
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
        .expect("focus a");
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
                text: "error[E0308]: mismatched types，帳單 NT$13,450".into(),
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
        fid
    }

    fn fake_cli(
        dir: &std::path::Path,
        json: &str,
        sentinel: &std::path::Path,
    ) -> (String, Vec<String>) {
        let script = dir.join("fake-brain.py");
        // 兩個方向都走 `.buffer`（bytes），不碰 Python 的文字層：它的編碼預設
        // 跟著作業系統的字碼頁走，開發機是 UTF-8，Windows 是 ANSI。
        // alpha.57 就是被 Windows CI 擋在這裡——prompt 和卡片裡都有中文，
        // `sys.stdout.write` 死在 UnicodeEncodeError，回來是空的 stdout → `BadJson`。
        // 而 `sys.stdin.read()` 在 cp1252 底下是解碼成亂碼還是直接爆，看的是
        // 那個字碼頁有沒有未定義的 byte——兩種都不是我們要測的東西。
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

    #[test]
    fn interpret_does_not_spawn_without_consent() {
        let dir =
            std::env::temp_dir().join(format!("sister-brain-noconsent-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sentinel = dir.join("spawned");
        let _ = std::fs::remove_file(&sentinel);
        let json = r#"{"segment_ref":"x","activity":"x","entities":[],"confidence":0.5,"evidence_refs":["frame:1"],"open_questions":[]}"#;
        let (command, args) = fake_cli(&dir, json, &sentinel);

        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_000_000_000;
        let fid = seed(&mut db, ts);
        let consent = Consent::default();
        let brain = crate::config::BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = InterpretInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            limit: 4,
            only_core_start: None,
        };
        let result = run(&mut input).expect("run");
        assert!(matches!(result.skip, Some(SkipReason::NoConsent)));
        assert!(!sentinel.exists(), "沒簽同意書 2 卻 spawn 了");
        assert!(
            db.latest_l2_for_segment(ts).expect("l2").is_none(),
            "沒簽不該寫卡片"
        );
        let _ = fid;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interpret_writes_a_card_through_a_fake_cli() {
        let dir = std::env::temp_dir().join(format!("sister-brain-ok-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sentinel = dir.join("spawned");
        let _ = std::fs::remove_file(&sentinel);

        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_000_100_000;
        let fid = seed(&mut db, ts);
        let segs = db.chapters_for_range(ts, ts + 400_000).expect("segs");
        assert!(!segs.is_empty(), "要切得出段落才測得到");
        let core = segs[0].core_started_at;
        let json = format!(
            r#"{{"segment_ref":"segment:{core}","activity":"在修 compiler error","entities":[],"confidence":0.55,"evidence_refs":["frame:{fid}"],"open_questions":["存了沒"]}}"#
        );
        let (command, args) = fake_cli(&dir, &json, &sentinel);
        let mut consent = Consent::default();
        consent.grant(Sheet::CloudReading, 1);
        let brain = crate::config::BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = InterpretInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            limit: 4,
            only_core_start: Some(core),
        };
        let result = run(&mut input).expect("run");
        assert!(result.skip.is_none(), "{:?}", result.skip);
        assert!(sentinel.exists(), "簽了卻沒 spawn");
        assert_eq!(result.ran.len(), 1);
        assert_eq!(result.ran[0].outcome, OutboundOutcome::Success);
        let card = db
            .latest_l2_for_segment(core)
            .expect("l2")
            .expect("written");
        assert_eq!(card.activity, "在修 compiler error");
        assert_eq!(card.model_confidence, 0.55);
        let logs = db.list_brain_outbound(10).expect("log");
        assert_eq!(logs.len(), 1);
        assert!(!logs[0].args_json.contains("13,450"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_shows_the_text_and_does_not_spawn() {
        let dir = std::env::temp_dir().join(format!("sister-brain-dry-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sentinel = dir.join("spawned");
        let _ = std::fs::remove_file(&sentinel);
        let mut db = Db::open_in_memory().expect("db");
        let ts = 1_700_000_200_000;
        let fid = seed(&mut db, ts);
        let (command, args) = fake_cli(&dir, "{}", &sentinel);
        let consent = Consent::default();
        let brain = crate::config::BrainConfig {
            command,
            args,
            ..Default::default()
        };
        let mut input = InterpretInput {
            db: &mut db,
            consent: &consent,
            brain: &brain,
            from_ts: ts,
            to_ts: ts + 400_000,
            limit: 4,
            only_core_start: None,
        };
        let report = prepare(&mut input).expect("prepare");
        let text = format_dry_run(&report);
        assert!(text.contains("不會送出去"), "{text}");
        // dry-run 的用處就是讓他在簽字前**看到真的會送出去的那段字**。
        // 不去敏，所以金額原封不動印出來——印成 `<AMT_1>` 反而是騙他。
        assert!(text.contains("13,450"), "dry-run 該印原文金額：{text}");
        assert!(
            !text.contains("<AMT_1>"),
            "已經不去敏了，不該有代號：{text}"
        );
        assert!(text.contains("沒簽"), "{text}");
        assert!(!sentinel.exists(), "dry-run 卻 spawn 了");
        let _ = fid;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_messages_are_not_the_same_sentence() {
        let a = SkipReason::NoConsent.message();
        let b = SkipReason::NoCommand.message();
        let c = SkipReason::BudgetExhausted {
            used: 80,
            limit: 80,
        }
        .message();
        let d = SkipReason::NothingWorthInterpreting { remaining: 80 }.message();
        let all = [&a, &b, &c, &d];
        for (i, x) in all.iter().enumerate() {
            for (j, y) in all.iter().enumerate() {
                if i != j {
                    assert_ne!(x, y, "兩種原因印成同一句話");
                }
            }
        }
        assert!(a.contains("同意書"));
        assert!(b.contains("[brain] command"));
        assert!(c.contains("預算"));
        assert!(d.contains("值得理解"));
    }
}
