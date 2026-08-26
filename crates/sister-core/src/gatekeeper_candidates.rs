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
    // SessionEnd 只是錄製容器收尾（連 60 秒 bench 都會寫），不是一天結束。
    // 真正能證明「日終」的是成功 ReviewKind::Eod run。40 分鐘是未量過的
    // 實作選擇；只收最新一列，不重複產生同一輪的 offer。
    if let Some(run) =
        db.latest_reviewer_eod_in_range(now.saturating_sub(40 * 60_000), now.saturating_add(1))?
    {
        // 這一天要跟寫日摘要的那一邊算出同一天，不然「筆記做好了」會配到
        // 另一天的 `daysummary:` id。所以不在這裡自己算，叫 reviewer 那份。
        let day = crate::reviewer::summarized_day(run.ts)
            .ok_or_else(|| anyhow::anyhow!("算不出 reviewer_run #{} 盤點的是哪一天", run.id))?;
        if let Some(candidate) = session_end_candidate(run.id, &day, day_note_state(db, &day)?)? {
            out.push(candidate);
        }
    }
    Ok(out)
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
    Candidate::new(
        SpeakCategory::CommitmentDue,
        format!("「{}」的時間快到了。", row.text),
        refs,
        0.9,
        row.confidence,
        0.9,
        1.0,
    )
    .map(Some)
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
            people_json: "[]".into(),
            due_hint: Some("17:00".into()),
            due_source: Some(source.into()),
            due_at: Some(10_000),
            status: "open".into(),
            confidence: 0.9,
            allowed_next_step: None,
            last_evidence_seen_at: None,
            kill_note: None,
            created_at: 1,
            updated_at: 1,
            tombstoned_at: None,
        };

        let explicit = super::commitment_candidate(&row("explicit")).unwrap();
        assert_eq!(explicit.unwrap().category, SpeakCategory::CommitmentDue);
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
}
