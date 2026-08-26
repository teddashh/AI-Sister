//! 主動開口白名單裡目前有訊號源的 a/c 類候選。
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
    Ok(out)
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

#[cfg(test)]
mod tests {
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
}
