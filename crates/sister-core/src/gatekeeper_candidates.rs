//! 主動開口白名單裡目前有訊號源的 a/c/d 類候選。
//!
//! 放在 core，理由和 [`crate::answer`] 相同：CLI 的預演與每天真的會用的字母人
//! 必須對同一份資料產生同一批候選。這裡刻意不讀 [`crate::followup`]；那是使用者
//! 先開口後才附在回答尾端的被動確認，不是主動開口來源。

use crate::Millis;
use crate::db::{CommitmentRow, Db};
use crate::gatekeeper::Candidate;
use crate::moments::SpeakCategory;
use crate::stuck::StuckSignal;

pub fn collect(db: &Db, now: Millis) -> anyhow::Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for row in db.open_commitments_due_before(now.saturating_add(40 * 60_000))? {
        if let Some(candidate) = commitment_candidate(&row)? {
            out.push(candidate);
        }
    }
    for signal in db.stuck_in_range(now.saturating_sub(40 * 60_000), now.saturating_add(1))? {
        out.push(stuck_candidate(&signal)?);
    }
    match session_end(db, now)? {
        DayEnd::Offer(candidate) => out.push(candidate),
        DayEnd::NoRecentEodRun | DayEnd::NothingToWriteAbout { .. } | DayEnd::Forgotten { .. } => {}
    }
    Ok(out)
}

/// d 類這一輪的結果。**四種，而其中三種都是「沒有候選」。**
///
/// 分開回，不折成 `Option<Candidate>`：預演那一頁要對使用者說明為什麼這一類
/// 這一輪沒有講話，而三種「沒有」的理由完全不同。折成一個 `None` 的話，
/// 那句說明只能是一句寫死的猜測——它會在他剛把那天的筆記忘掉之後，
/// 告訴他「沒有最近的日終盤點」，而盤點一分鐘前才跑完。
// 沒有 `Eq`：`Candidate` 的四個因子是 `f64`。
#[derive(Debug, Clone, PartialEq)]
pub enum DayEnd {
    /// 最近 40 分鐘沒有成功的日終盤點。
    NoRecentEodRun,
    Offer(Candidate),
    NothingToWriteAbout {
        day: String,
    },
    Forgotten {
        day: String,
    },
}

impl DayEnd {
    /// 這一輪為什麼沒有 d 類候選。有候選的時候回 `None`。
    pub fn why_silent(&self) -> Option<String> {
        match self {
            Self::Offer(_) => None,
            Self::NoRecentEodRun => {
                Some("session_end：最近 40 分鐘沒有跑成功的日終盤點。".to_string())
            }
            Self::NothingToWriteAbout { day } => Some(format!(
                "session_end：{day} 一張 L2 卡都沒有，寫不出筆記，所以不問。"
            )),
            Self::Forgotten { day } => Some(format!(
                "session_end：{day} 的筆記被忘掉了，不提議把它寫回來。"
            )),
        }
    }
}

/// d 類：日終「要不要做筆記」。
///
/// `SystemKind::SessionEnd` **不是**訊號源——那只證明一場錄製容器收尾，
/// 連 60 秒 bench 都會寫一列。真正能證明「這一天結束了」的是一輪成功的
/// [`crate::reviewer::ReviewKind::Eod`]。40 分鐘是未量過的實作選擇；
/// 只收最新一列，不重複產生同一輪的 offer。
pub fn session_end(db: &Db, now: Millis) -> anyhow::Result<DayEnd> {
    let Some(run) =
        db.latest_reviewer_eod_in_range(now.saturating_sub(40 * 60_000), now.saturating_add(1))?
    else {
        return Ok(DayEnd::NoRecentEodRun);
    };
    // 這一天要跟寫日摘要的那一邊算出同一天，不然「筆記做好了」會配到
    // 另一天的 `daysummary:` id。所以不在這裡自己算，叫 reviewer 那份。
    let day = crate::reviewer::summarized_day(run.ts)
        .ok_or_else(|| anyhow::anyhow!("算不出 reviewer_run #{} 盤點的是哪一天", run.id))?;
    let state = day_note_state(db, &day)?;
    Ok(match state {
        DayNoteState::NothingToWriteAbout => DayEnd::NothingToWriteAbout { day },
        DayNoteState::Forgotten { .. } => DayEnd::Forgotten { day },
        DayNoteState::Written { .. } | DayNoteState::Writable => {
            let candidate = session_end_candidate(run.id, &day, state)?
                .ok_or_else(|| anyhow::anyhow!("{day} 的筆記狀態說開得了口，卻沒有產出候選"))?;
            DayEnd::Offer(candidate)
        }
    })
}

/// 被盤點的那一天，筆記現在是什麼狀況。
///
/// **四種，不是兩種。** `latest_day_summary()` 回 `None`同時蓋著三件相反的事：
/// 那天沒有素材、那天有素材但還沒寫、以及**使用者剛剛親手把它忘掉了**。
/// `db.rs` 那支查詢自己的註解就寫著「墓碑列故意不回：拿來當『這一天的摘要』
/// 會把『被刪掉了』講成『那天什麼都沒發生』」——這裡是同一個坑的下游。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayNoteState {
    /// 摘要寫好了，活著。
    Written { summary_id: i64 },
    /// 沒有摘要，但那天有 L2 卡——寫得出來。
    Writable,
    /// 那天一張 L2 卡都沒有。**寫不出來**，所以不要開口問要不要寫。
    NothingToWriteAbout,
    /// 他自己把那天的筆記忘掉了。不要提議把它重建回來。
    ///
    /// 今天只有一條路走得到這裡：`forget` 的血緣 cascade，而那條路會**連同
    /// 那天的 L2 卡一起**清掉，所以它和 [`Self::NothingToWriteAbout`] 現在
    /// 收在同一個結局（都不開口）。分成兩格是為了下一個人：等畫面上多一顆
    /// 「只忘掉這份筆記」的按鈕，L2 卡就會活著，而那時候走進哪一格會決定她
    /// 要不要問「要不要我把它寫回來」。
    Forgotten { tombstoned_at: Millis },
}

fn day_note_state(db: &Db, day: &str) -> anyhow::Result<DayNoteState> {
    if let Some(row) = db.latest_day_summary(day)? {
        return Ok(DayNoteState::Written { summary_id: row.id });
    }
    if let Some(at) = db.day_summary_tombstoned_at(day)? {
        return Ok(DayNoteState::Forgotten { tombstoned_at: at });
    }
    let (from_ts, to_ts) = crate::local_day::local_day_bounds(day)
        .ok_or_else(|| anyhow::anyhow!("算不出 {day} 的本地日界"))?;
    if db.l2_in_range(from_ts, to_ts)?.is_empty() {
        return Ok(DayNoteState::NothingToWriteAbout);
    }
    Ok(DayNoteState::Writable)
}

fn commitment_candidate(row: &CommitmentRow) -> anyhow::Result<Option<Candidate>> {
    if row.due_source.as_deref() != Some("explicit") {
        return Ok(None);
    }
    let evidence: Vec<String> = serde_json::from_str(&row.evidence_json).unwrap_or_default();
    let mut refs = vec![format!("commitment:{}", row.id)];
    refs.extend(evidence);
    let mut candidate = Candidate::new(
        SpeakCategory::CommitmentDue,
        format!("「{}」的時間快到了。", row.text),
        refs,
        0.9,
        row.confidence,
        0.9,
        1.0,
    )?;
    candidate.commitment_id = Some(row.id);
    Ok(Some(candidate))
}

/// 一張已經存下來的話，evidence 裡指到哪一張承諾卡。
///
/// **三個答案，不是兩個。** 「這張卡根本不是在講承諾」和「這張卡同時指到兩張
/// 承諾、我不猜是哪一張」在畫面上會長得一樣（都沒有按鈕），但只有後者是需要
/// 有人去看一眼的異常。折成同一個 `None` 的那天，第二種就安靜地消失了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitmentRef {
    /// evidence 裡沒有 `commitment:` ——這張卡講的不是承諾。
    None,
    One(i64),
    /// 指到不只一張。**不猜。**
    Ambiguous {
        ids: Vec<i64>,
    },
}

impl CommitmentRef {
    /// 只認完整的 `commitment:<i64>` schema，不在文案裡找數字。
    pub fn from_evidence(evidence: &[String]) -> Self {
        let ids: Vec<i64> = evidence
            .iter()
            .filter_map(|value| {
                value
                    .strip_prefix("commitment:")
                    .and_then(|id| id.parse::<i64>().ok())
            })
            .collect();
        match ids.len() {
            0 => Self::None,
            1 => Self::One(ids[0]),
            _ => Self::Ambiguous { ids },
        }
    }

    /// 候選是活的時候，是誰生的它本來就記著，不必從 evidence 反推。
    pub fn from_candidate(commitment_id: Option<i64>) -> Self {
        match commitment_id {
            Some(id) => Self::One(id),
            None => Self::None,
        }
    }

    /// 為什麼這張卡上沒有那顆按鈕——沒什麼好解釋的時候回 `None`。
    pub fn why_no_button(&self) -> Option<String> {
        match self {
            // 這張卡不是在講承諾，本來就不會有下一步可做。
            Self::None => None,
            Self::One(_) => None,
            Self::Ambiguous { ids } => Some(format!(
                "這句話同時指到 {} 張承諾（{}），不知道按鈕該對哪一張，所以不放按鈕。",
                ids.len(),
                ids.iter()
                    .map(|id| format!("#{id}"))
                    .collect::<Vec<_>>()
                    .join("、")
            )),
        }
    }
}

fn stuck_candidate(signal: &StuckSignal) -> anyhow::Result<Candidate> {
    Candidate::new(
        SpeakCategory::Stuck,
        format!(
            "你似乎卡在{}同一個錯誤。",
            signal
                .app
                .as_deref()
                .map(|app| format!(" {app} 的"))
                .unwrap_or_default()
        ),
        vec![format!("segment:{}", signal.started_at)],
        0.6,
        0.8,
        0.8,
        0.8,
    )
}

/// `2026-08-25` → `8/25`。講給人聽的時候不念年份。
///
/// 看不懂的字串原樣回去。這一格寧可講出一個醜的 `2026-08-25`，
/// 也不要猜一個好看的日期——猜錯的那一天她會用很自然的語氣講出來。
fn spoken_day(day: &str) -> String {
    match chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d") {
        Ok(date) => format!("{}/{}", date.format("%-m"), date.format("%-d")),
        Err(_) => day.to_string(),
    }
}

fn session_end_candidate(
    reviewer_run_id: i64,
    day: &str,
    state: DayNoteState,
) -> anyhow::Result<Option<Candidate>> {
    // 講日期，不講「今天」。日終盤點盤的是 `previous_local_day_key`，而它跑
    // 完的時候多半已經過午夜了——那一刻的「今天」是**還沒有筆記**的那一天。
    let spoken = spoken_day(day);
    let (text, evidence) = match state {
        DayNoteState::Written { summary_id } => (
            format!("{spoken} 的筆記做好了，要看嗎？"),
            vec![
                format!("reviewer_run:{reviewer_run_id}"),
                format!("daysummary:{summary_id}"),
            ],
        ),
        DayNoteState::Writable => (
            format!("要不要我把 {spoken} 寫成筆記？"),
            vec![format!("reviewer_run:{reviewer_run_id}")],
        ),
        // 那天一張 L2 卡都沒有。答應了也只會生出一份空的東西，
        // 而「她問了、你說好、然後什麼都沒有」比她不問更糟。
        DayNoteState::NothingToWriteAbout => return Ok(None),
        // 他剛按過「忘記」。提議把它寫回來等於問他要不要撤銷自己的刪除。
        DayNoteState::Forgotten { .. } => return Ok(None),
    };
    // 四個數字都是實作選擇，尚未用 replay 量過。impact=0.5：只是可忽略的
    // 筆記 offer；confidence=0.95：EOD run 與摘要列是程式寫的，但不把
    // 「今天已完全結束」當成 1.0；timeliness=0.9：只收 40 分鐘內；
    // evidence_strength=0.95：有真實 reviewer_run row，有摘要時再加摘要 row。
    Candidate::new(
        SpeakCategory::SessionEnd,
        text,
        evidence,
        0.5,
        0.95,
        0.9,
        0.95,
    )
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::DayNoteState;
    use crate::db::CommitmentRow;
    use crate::moments::SpeakCategory;

    #[test]
    fn explicit_commitment_becomes_a_candidate_but_inferred_does_not() {
        let row = |source: &str| CommitmentRow {
            id: 7,
            text: "五點去接她".into(),
            kind: "reminder".into(),
            born_from: 1,
            evidence_json: r#"[\"frame:42\"]"#.into(),
            agreed_evidence_json: Some(r#"[\"frame:42\"]"#.into()),
            people_json: "[]".into(),
            due_hint: Some("17:00".into()),
            due_source: Some(source.into()),
            due_at: Some(10_000),
            status: "open".into(),
            confidence: 0.9,
            allowed_next_step: None,
            allowed_next_step_fact: None,
            last_evidence_seen_at: None,
            kill_note: None,
            created_at: 1,
            updated_at: 1,
            tombstoned_at: None,
        };

        let explicit = super::commitment_candidate(&row("explicit"))
            .unwrap()
            .unwrap();
        assert_eq!(explicit.category, SpeakCategory::CommitmentDue);
        assert_eq!(explicit.commitment_id, Some(7));
        assert!(
            super::commitment_candidate(&row("inferred"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn stuck_candidate_says_stuck_and_does_not_claim_completion() {
        let signal = crate::stuck::StuckSignal {
            started_at: 5,
            ended_at: 10,
            app: Some("Code".into()),
            title: None,
            dwell_ms: 5,
            switch_count: 3,
            error_fact_count: 2,
        };
        let candidate = super::stuck_candidate(&signal).unwrap();
        assert!(candidate.text.contains("卡"));
        assert!(!candidate.text.contains("完成"));
    }

    #[test]
    fn session_end_with_summary_says_note_is_ready_not_offer_to_make_one() {
        let candidate = super::session_end_candidate(
            31,
            "2026-08-25",
            DayNoteState::Written { summary_id: 44 },
        )
        .unwrap()
        .expect("寫好的筆記要開得了口");
        assert!(candidate.text.contains("筆記做好了"));
        assert!(candidate.text.contains("要看嗎"));
        assert!(!candidate.text.contains("要不要我把"));
        assert!(candidate.evidence.contains(&"reviewer_run:31".to_string()));
        assert!(candidate.evidence.contains(&"daysummary:44".to_string()));
    }

    #[test]
    fn session_end_without_summary_offers_to_make_note_not_claim_it_is_ready() {
        let candidate = super::session_end_candidate(31, "2026-08-25", DayNoteState::Writable)
            .unwrap()
            .expect("有素材沒摘要要問一句");
        assert!(candidate.text.contains("要不要我把"));
        assert!(candidate.text.contains("寫成筆記"));
        assert!(!candidate.text.contains("筆記做好了"));
        assert!(!candidate.text.contains("要看嗎"));
        assert_eq!(candidate.evidence, ["reviewer_run:31"]);
    }

    /// 日終盤點是**過了午夜之後**跑的，盤的是 `previous_local_day_key`。
    /// 所以那一刻的「今天」正是還沒有筆記的那一天，而她講的是前一天。
    /// 兩句話都不可以出現「今天」。
    #[test]
    fn the_day_end_sentences_name_the_date_and_never_say_today() {
        for state in [
            DayNoteState::Written { summary_id: 44 },
            DayNoteState::Writable,
        ] {
            let candidate = super::session_end_candidate(31, "2026-08-25", state)
                .unwrap()
                .expect("這兩種狀態都要開得了口");
            assert!(
                candidate.text.contains("8/25"),
                "要講出是哪一天：{}",
                candidate.text
            );
            assert!(
                !candidate.text.contains("今天"),
                "跑完盤點的時候「今天」已經是下一天了：{}",
                candidate.text
            );
        }
    }

    /// 一件二十分鐘後到期的事該不該講，跟那一列被寫過幾次沒有關係。
    ///
    /// `open_commitments_due_before` 曾經多一句 `AND updated_at = created_at`
    /// ——那是 `archive_overdue` 用來分辨「到期後有沒有人碰過」的條件，
    /// 在**這支**查詢裡問的是另一個問題。它今天剛好篩不掉東西，
    /// 所以誰都不會發現，直到有人把 `last_evidence_seen_at` 接起來。
    #[test]
    fn a_commitment_someone_has_written_to_is_still_announced() {
        let mut db = crate::db::Db::open_in_memory().unwrap();
        let now = 1_777_000_000_000;
        let id = db
            .insert_commitment(
                crate::reviewer::test_l3_write(),
                &crate::db::CommitmentInsert {
                    text: "把 Cloudflare DNS 改完",
                    kind: "commitment",
                    born_from: 1,
                    evidence_json: "[]".into(),
                    agreed_evidence_json: Some("[]".into()),
                    people_json: "[]".into(),
                    due_hint: Some("五點"),
                    due_source: Some("explicit"),
                    due_at: Some(now + 20 * 60_000),
                    status: "open",
                    confidence: 0.9,
                    allowed_next_step: None,
                    allowed_next_step_fact: None,
                    last_evidence_seen_at: None,
                    kill_note: None,
                    now,
                },
            )
            .unwrap();
        // 有人碰過這一列（`updated_at` 被 bump），但它還是開著、
        // 還是二十分鐘後到期。
        db.update_commitment_status(crate::reviewer::test_l3_write(), id, "open", None, now + 1)
            .unwrap();

        let texts: Vec<String> = super::collect(&db, now)
            .unwrap()
            .into_iter()
            .filter(|c| c.category == SpeakCategory::CommitmentDue)
            .map(|c| c.text)
            .collect();
        assert_eq!(texts.len(), 1, "被寫過的那一列不該消失：{texts:?}");
        assert!(texts[0].contains("Cloudflare DNS"));
    }

    /// 三種「沒有 d 類候選」的理由**要說得出是哪一種**。
    ///
    /// 這一條抓的是一句寫死的說明：它原本無論如何都印「d 類沒有最近 40 分鐘
    /// 的成功日終盤點」，而在他剛把那天的筆記忘掉的時候，盤點一分鐘前才跑完。
    #[test]
    fn each_silent_day_end_says_its_own_reason() {
        use super::DayEnd;
        let no_run = DayEnd::NoRecentEodRun.why_silent().unwrap();
        let nothing = DayEnd::NothingToWriteAbout {
            day: "2026-08-25".into(),
        }
        .why_silent()
        .unwrap();
        let forgotten = DayEnd::Forgotten {
            day: "2026-08-25".into(),
        }
        .why_silent()
        .unwrap();

        assert!(no_run.contains("沒有跑成功的日終盤點"));
        assert!(!no_run.contains("2026-08-25"), "沒有 run 就講不出是哪一天");

        assert!(nothing.contains("2026-08-25"));
        assert!(nothing.contains("寫不出"));
        assert!(
            !nothing.contains("日終盤點"),
            "盤點跑過了，不可以說沒跑：{nothing}"
        );

        assert!(forgotten.contains("2026-08-25"));
        assert!(forgotten.contains("忘掉"));
        assert!(
            !forgotten.contains("日終盤點"),
            "盤點跑過了，不可以說沒跑：{forgotten}"
        );

        assert_ne!(nothing, forgotten);
        assert_ne!(no_run, nothing);
    }

    /// 有候選的時候不可以再印一句「為什麼沒講話」。
    #[test]
    fn an_offer_has_nothing_to_explain() {
        let candidate =
            super::session_end_candidate(31, "2026-08-25", DayNoteState::Writable).unwrap();
        assert!(
            super::DayEnd::Offer(candidate.unwrap())
                .why_silent()
                .is_none()
        );
    }

    /// 兩種「沒有摘要」是相反的兩件事，而且**都不該開口**——一個是答應了也
    /// 生不出東西，一個是他五分鐘前才親手把那天忘掉。
    #[test]
    fn nothing_to_write_about_and_forgotten_both_stay_quiet() {
        assert!(
            super::session_end_candidate(31, "2026-08-25", DayNoteState::NothingToWriteAbout)
                .unwrap()
                .is_none()
        );
        assert!(
            super::session_end_candidate(
                31,
                "2026-08-25",
                DayNoteState::Forgotten {
                    tombstoned_at: 1_777_000_000_000
                }
            )
            .unwrap()
            .is_none()
        );
    }

    fn eod_run(db: &mut crate::db::Db, now: crate::Millis) -> i64 {
        db.insert_reviewer_run(&crate::db::ReviewerRunInsert {
            ts: now - 1_000,
            day_key: &crate::local_day::local_day_key(now).unwrap(),
            kind: "eod",
            skip_reason: None,
            candidate_count: Some(0),
            recheck_count: Some(0),
            wrote_commitments: 0,
            divergences: 0,
            calls_used: 0,
            budget_used: 0,
            budget_limit: 0,
            detail: "",
            notes: "",
            answers_got: None,
        })
        .unwrap()
    }

    /// 被盤點的那一天塞一張 L2 卡，回傳那一天的 day key。
    fn a_card_on_the_summarized_day(db: &mut crate::db::Db, now: crate::Millis) -> String {
        let day = crate::local_day::previous_local_day_key(now).unwrap();
        let (from_ts, _) = crate::local_day::local_day_bounds(&day).unwrap();
        db.insert_l2_card(&crate::db::L2Insert {
            segment_core_start: from_ts + 3_600_000,
            segment_ref: "segment:1",
            activity: "在改 DNS 設定",
            entities_json: "[]".into(),
            continues_json: None,
            commitments_json: "[]".into(),
            model_confidence: 0.8,
            evidence_json: "[]".into(),
            open_questions_json: "[]".into(),
            author: crate::db::L2Author::Interpreter,
        })
        .unwrap();
        day
    }

    /// 一輪成功的日終盤點 + 那天有素材 = 一句「要不要我把 X 寫成筆記」。
    #[test]
    fn collect_gets_d_from_a_successful_recent_eod_run() {
        let mut db = crate::db::Db::open_in_memory().unwrap();
        let now = 1_777_000_000_000;
        let run_id = eod_run(&mut db, now);
        a_card_on_the_summarized_day(&mut db, now);

        let candidates = super::collect(&db, now).unwrap();
        let d = candidates
            .iter()
            .find(|c| c.category == SpeakCategory::SessionEnd)
            .expect("successful recent EOD must enter collect");
        assert!(d.text.contains("寫成筆記"));
        assert!(!d.text.contains("筆記做好了"));
        assert_eq!(d.evidence, [format!("reviewer_run:{run_id}")]);
    }

    /// **這一條是上一條的另一半。** 上一條原本沒有塞 L2 卡也會過，因為
    /// `latest_day_summary()` 和「那天有沒有素材」當時都回同一個 `None`——
    /// 一句「要不要我把今天寫成筆記」會在一天完全沒有素材的時候問出口，
    /// 而答應了她只生得出一份空的。
    #[test]
    fn a_day_with_no_material_produces_no_offer_to_write_it_up() {
        let mut db = crate::db::Db::open_in_memory().unwrap();
        let now = 1_777_000_000_000;
        eod_run(&mut db, now);
        // 刻意不塞 L2 卡。
        let candidates = super::collect(&db, now).unwrap();
        assert!(
            !candidates
                .iter()
                .any(|c| c.category == SpeakCategory::SessionEnd),
            "沒有素材的那一天不該提議寫筆記：{:?}",
            candidates.iter().map(|c| &c.text).collect::<Vec<_>>()
        );
    }

    /// 她講的那一天，要**等於**寫日摘要的那一邊盤的那一天。
    ///
    /// 這一條看起來像在測一句廢話，它守的是別的事：`collect` 曾經自己算過
    /// 一次 `previous_local_day_key`。兩份副本裡有一份改成 `local_day_key`
    /// 的那一天，她會說「8/26 的筆記做好了」然後把 8/25 的那一列端出來——
    /// 每一行都是真的。現在只剩一份定義，這條測試釘的是它沒有再長出第二份。
    #[test]
    fn the_day_she_names_is_the_day_the_summary_was_written_for() {
        let now = 1_777_000_000_000;
        let day = crate::reviewer::summarized_day(now).unwrap();
        let candidate =
            super::session_end_candidate(31, &day, DayNoteState::Written { summary_id: 44 })
                .unwrap()
                .unwrap();
        assert!(candidate.text.contains(&super::spoken_day(&day)));
        assert_ne!(day, crate::local_day::local_day_key(now).unwrap());
    }

    /// 「不是承諾」和「同時指到兩張承諾」不能是同一個答案。
    ///
    /// 兩種在畫面上都是「沒有按鈕」，所以看畫面分不出來。差別在開發者那一欄：
    /// 只有第二種要留下一句話。折回 `Option<i64>` 的那天，第二種會安靜地消失，
    /// 而它正是那種需要有人去看一眼的狀況。
    #[test]
    fn a_card_naming_two_commitments_is_not_a_card_naming_none() {
        use super::CommitmentRef;
        let none = CommitmentRef::from_evidence(&["frame:1".into(), "segment:9".into()]);
        assert_eq!(none, CommitmentRef::None);
        assert_eq!(none.why_no_button(), None);

        let one = CommitmentRef::from_evidence(&["commitment:7".into(), "frame:1".into()]);
        assert_eq!(one, CommitmentRef::One(7));
        assert_eq!(one.why_no_button(), None);

        let two = CommitmentRef::from_evidence(&["commitment:7".into(), "commitment:8".into()]);
        assert_eq!(two, CommitmentRef::Ambiguous { ids: vec![7, 8] });
        let why = two.why_no_button().expect("兩張承諾要留下一句話");
        assert!(why.contains("#7") && why.contains("#8"), "{why}");

        // `commitment:` 只認完整 schema，不在文案裡撈數字。
        assert_eq!(
            CommitmentRef::from_evidence(&["承諾 7 快到了".into(), "commitment:x".into()]),
            CommitmentRef::None
        );
    }
}
