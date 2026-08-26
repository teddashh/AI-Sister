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

/// 守門員是被輪詢的。同一件事被問第二次，要找得到第一次那一列。
///
/// 沒有這一條的話，字母人每 5 秒問一次就記一列、每一列都算一次點數——
/// 「每天 5 點」量到的就變成畫面重新整理了幾次，25 秒燒完，而人一句都還
/// 沒看到。這條測試釘的是「認得出同一件事」，不是「能不能寫進去」。
#[test]
fn the_same_thing_asked_twice_today_finds_the_first_row() {
    let mut db = db();
    let c = candidate();
    let spoke = Verdict::Speak {
        form: Form::OneLine,
        cost: 1,
    };
    let id = db
        .record_utterance(&UtteranceInsert {
            ts: 1,
            day_key: "2026-08-26",
            candidate: &c,
            verdict: &spoke,
        })
        .unwrap();

    let found = db
        .utterance_today_for("2026-08-26", c.category, &c.evidence)
        .unwrap()
        .expect("同一天、同一類、同一份 evidence 要找得到");
    assert_eq!(found.id, id);
    assert!(found.reaction.is_none(), "還沒有人回應過它");

    // 換一天就是另一件事——預算是每天結算的。
    assert!(
        db.utterance_today_for("2026-08-27", c.category, &c.evidence)
            .unwrap()
            .is_none()
    );
    // 換一份 evidence 也是另一件事：evidence 跟著那張承諾走，不跟著
    // 「我第幾次去問」走。
    assert!(
        db.utterance_today_for("2026-08-26", c.category, &["commitment:8".to_string()])
            .unwrap()
            .is_none()
    );
    // 而它真的擋得住重複計費：第二次問沒有寫進去，點數還是 1。
    assert_eq!(db.points_spent_today("2026-08-26").unwrap(), 1);
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
