use sister_core::db::{Db, UtteranceDecision, UtteranceInsert};
use sister_core::gatekeeper::{Candidate, Form, HoldReason, Verdict};
use sister_core::moments::SpeakCategory;

fn db() -> Db {
    Db::open_in_memory().expect("db")
}
fn candidate() -> Candidate {
    Candidate::new(
        SpeakCategory::CommitmentDue,
        "五點的承諾快到了。",
        vec!["commitment:7".into()],
        1.0,
        0.9,
        0.9,
        1.0,
    )
    .unwrap()
}

#[test]
fn utterance_budget_counts_points_not_rows_and_held_costs_zero() {
    let mut db = db();
    let c = candidate();
    let card = Verdict::Speak {
        form: Form::Card,
        cost: 2,
    };
    db.record_utterance(&UtteranceInsert {
        ts: 1,
        day_key: "2026-08-26",
        candidate: &c,
        verdict: &card,
    })
    .unwrap();
    let held = Verdict::Hold(HoldReason::MissingEvidence);
    db.record_utterance(&UtteranceInsert {
        ts: 2,
        day_key: "2026-08-26",
        candidate: &c,
        verdict: &held,
    })
    .unwrap();
    assert_eq!(db.points_spent_today("2026-08-26").unwrap(), 2);
    assert!(matches!(
        db.utterances_on_day("2026-08-26").unwrap()[1].decision,
        UtteranceDecision::Held { .. }
    ));
}

#[test]
fn first_utterance_query_counts_only_rows_that_really_spoke() {
    let mut db = db();
    let c = candidate();
    let held = Verdict::Hold(HoldReason::MissingEvidence);
    db.record_utterance(&UtteranceInsert {
        ts: 1,
        day_key: "2026-08-26",
        candidate: &c,
        verdict: &held,
    })
    .unwrap();
    assert!(!db.has_ever_spoken().unwrap());
    let spoke = Verdict::Speak {
        form: Form::OneLine,
        cost: 1,
    };
    db.record_utterance(&UtteranceInsert {
        ts: 2,
        day_key: "2026-08-26",
        candidate: &c,
        verdict: &spoke,
    })
    .unwrap();
    assert!(db.has_ever_spoken().unwrap());
}
