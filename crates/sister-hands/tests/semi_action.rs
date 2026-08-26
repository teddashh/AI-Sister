use sister_hands::semi_action::*;
use sister_hands::{
    ActionEvent, ActionSnapshot, Executor, NeverInherited, Outcome, RefusalReason, Suggestion,
    SuggestionButton,
};
use std::path::PathBuf;

/// 唯一在意的事：作業系統到底被碰過幾次。
#[derive(Default)]
struct CountingExecutor {
    calls: usize,
}
impl Executor for CountingExecutor {
    fn execute(&mut self, _suggestion: &Suggestion) -> Result<String, String> {
        self.calls += 1;
        Ok("開了".into())
    }
}

fn pressed(json: &str) -> Suggestion {
    SuggestionButton::parse_json(json).unwrap().press()
}

fn action() -> ActionSnapshot {
    ActionSnapshot::OpenFile {
        path: PathBuf::from("C:/work/a.txt"),
    }
}

fn grant() -> Grant {
    Grant::new(
        Task::new("整理報告"),
        AllowedApps::new([App::new("Editor")]),
        AllowedActions::new([ActionKind::OpenFile]),
        Expiry::after_issued(1_000, 300_000),
        StepLimit::new(2).unwrap(),
    )
}

#[test]
fn grant_rejection_names_each_blocking_dimension() {
    let grant = grant();
    let cases = [
        (
            StepRequest::new(Task::new("寄信"), App::new("Editor"), action()),
            1_001,
            "task",
            "apps",
        ),
        (
            StepRequest::new(Task::new("整理報告"), App::new("Mail"), action()),
            1_001,
            "apps",
            "actions",
        ),
        (
            StepRequest::new(
                Task::new("整理報告"),
                App::new("Editor"),
                ActionSnapshot::OpenUrl {
                    url: "https://x".into(),
                },
            ),
            1_001,
            "actions",
            "expiry",
        ),
        (
            StepRequest::new(Task::new("整理報告"), App::new("Editor"), action()),
            301_001,
            "expiry",
            "task",
        ),
    ];
    for (step, now, must, must_not) in cases {
        let text = grant.covers(&step, now).unwrap_err().message();
        assert!(text.contains(must), "{text}");
        assert!(!text.contains(must_not), "{text}");
    }
}

#[test]
fn clock_rollback_is_an_expiry_refusal_not_fresh_time() {
    let text = grant()
        .covers(
            &StepRequest::new(Task::new("整理報告"), App::new("Editor"), action()),
            999,
        )
        .unwrap_err()
        .message();
    assert!(text.contains("expiry"));
    assert!(text.contains("倒退"));
    assert!(!text.contains("仍有效"));
}

#[test]
fn approval_for_a_cannot_authorize_b() {
    let shown = PresentedStep::new(StepRequest::new(
        Task::new("整理報告"),
        App::new("Editor"),
        action(),
    ));
    let approval = shown.approve();
    let b = StepRequest::new(
        Task::new("整理報告"),
        App::new("Editor"),
        ActionSnapshot::OpenFile {
            path: PathBuf::from("C:/work/b.txt"),
        },
    );
    let text = approval.authorizes(&b).unwrap_err().message();
    assert!(text.contains("顯示的那一步"));
    assert!(text.contains("a.txt"));
    assert!(text.contains("b.txt"));
    assert!(!text.contains("已核准"));
}

#[test]
fn inherited_scope_and_separate_approval_are_different_questions() {
    let step = StepRequest::new(Task::new("整理報告"), App::new("Editor"), action());
    assert!(grant().covers(&step, 1_001).is_ok());
    assert_eq!(step.separate_approval_required(), None);
    assert_eq!(
        separate_approval_for_class(NeverInherited::Pay),
        SeparateApproval::Required(NeverInherited::Pay)
    );
}

#[test]
fn step_limit_and_completed_are_named_distinctly() {
    assert!(RunConclusion::Completed.message().contains("問完"));
    assert!(!RunConclusion::Completed.message().contains("上限"));
    // 「任務做完了」是假話：他可以每一步都說不要，這一輪照樣走到底。
    assert!(!RunConclusion::Completed.message().contains("做完"));
    assert!(
        RunConclusion::StepLimitReached {
            completed_steps: 2,
            limit: StepLimit::new(2).unwrap()
        }
        .message()
        .contains("上限")
    );
    assert!(
        !RunConclusion::StepLimitReached {
            completed_steps: 2,
            limit: StepLimit::new(2).unwrap()
        }
        .message()
        .contains("做完")
    );
}

#[test]
fn abort_log_names_step_and_who_stopped_it() {
    let event = ActionEvent::Aborted {
        at_ms: 9,
        after_completed_steps: 1,
        by: AbortActor::User,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("aborted"));
    assert!(json.contains("after_completed_steps"));
    assert!(json.contains("user"));
    assert!(!json.contains("completed\""));
}

#[test]
fn every_step_log_distinguishes_legacy_unchecked_from_checked_evidence() {
    let event = ActionEvent::StepFinished {
        at_ms: 9,
        step_number: 1,
        action: action(),
        evidence: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("evidence"));
    assert!(json.contains("null"));
    assert!(!json.contains("verified"));
    let with = ActionEvent::StepFinished {
        at_ms: 10,
        step_number: 2,
        action: action(),
        evidence: Some(StepEvidence::After {
            frame_id: 42,
            frame_at_ms: 10,
            has_image: true,
        }),
    };
    let json = serde_json::to_string(&with).unwrap();
    assert!(json.contains("\"kind\":\"after\""));
    assert!(json.contains("\"frame_id\":42"));
    assert!(!json.contains("null"));
}

/// 授權沒過的那幾種，`Outcome` 必須是 `Refused` 而不是 `Failed`。
///
/// 「她不肯做」和「她做了但失敗了」是兩件事：後者作業系統碰過了、而且不知道
/// 碰到哪一步。畫面上那一句 `Failed` 的文案是「她動手了，但執行失敗」——
/// 一次連 executor 都沒被呼叫的拒絕，用那一句講出來就是一句假話。
#[test]
fn an_unauthorized_step_is_refused_and_never_reaches_the_operating_system() {
    let suggestion = pressed(r#"{"action":"open_file","path":"C:/work/a.txt"}"#);
    let approved = StepRequest::new(Task::new("整理報告"), App::new("Editor"), action());

    let cases: Vec<(&str, StepRequest, i64, StepRequest)> = vec![
        (
            "task 不合",
            StepRequest::new(Task::new("寄信"), App::new("Editor"), action()),
            1_001,
            approved.clone(),
        ),
        ("過期", approved.clone(), 301_001, approved.clone()),
        (
            "票是對另一步簽的",
            approved.clone(),
            1_001,
            StepRequest::new(
                Task::new("整理報告"),
                App::new("Editor"),
                ActionSnapshot::OpenFile {
                    path: PathBuf::from("C:/work/b.txt"),
                },
            ),
        ),
    ];

    for (name, requested, now, shown) in cases {
        let mut executor = CountingExecutor::default();
        let outcome = execute_approved_step(
            &grant(),
            now,
            PresentedStep::new(shown).approve(),
            &requested,
            &mut executor,
            &suggestion,
        );
        assert!(
            matches!(outcome, Outcome::Refused { .. }),
            "{name}：{outcome:?}"
        );
        assert_eq!(executor.calls, 0, "{name}：executor 被呼叫了");
    }
}

/// 送給 executor 的那一步，和核准的那一步不同時也要擋下來。
#[test]
fn the_thing_handed_to_the_executor_must_be_the_thing_that_was_approved() {
    let step = StepRequest::new(Task::new("整理報告"), App::new("Editor"), action());
    let mut executor = CountingExecutor::default();
    let outcome = execute_approved_step(
        &grant(),
        1_001,
        PresentedStep::new(step.clone()).approve(),
        &step,
        &mut executor,
        &pressed(r#"{"action":"open_file","path":"C:/work/OTHER.txt"}"#),
    );
    let Outcome::Refused {
        reason: RefusalReason::ApprovalWasForAnotherStep { mismatch },
    } = &outcome
    else {
        panic!("{outcome:?}");
    };
    let text = mismatch.message();
    assert!(
        text.contains("a.txt") && text.contains("OTHER.txt"),
        "{text}"
    );
    assert_eq!(executor.calls, 0);
}

/// 全部對得上的時候才真的動手，而且動手的結局不叫「拒絕」。
#[test]
fn a_step_that_matches_on_every_dimension_actually_runs() {
    let step = StepRequest::new(Task::new("整理報告"), App::new("Editor"), action());
    let mut executor = CountingExecutor::default();
    let outcome = execute_approved_step(
        &grant(),
        1_001,
        PresentedStep::new(step.clone()).approve(),
        &step,
        &mut executor,
        &pressed(r#"{"action":"open_file","path":"C:/work/a.txt"}"#),
    );
    assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
    assert_eq!(executor.calls, 1);
}

/// 五類永不繼承的動作，`separate_approval_required` 那一份規則也要被隘口讀到。
///
/// 這一條釘的是「有沒有人讀」，不是「答案是什麼」：今天三種動作都不在那五類裡，
/// 所以只能證明隘口確實把兩份規則都問過一次。第四種動作進來時，兩份 match
/// 都會編譯錯誤。
#[test]
fn the_gate_asks_both_copies_of_the_never_inherited_rule() {
    let step = StepRequest::new(Task::new("整理報告"), App::new("Editor"), action());
    assert_eq!(step.separate_approval_required(), None);
    assert!(!sister_hands::is_never_inherited(&pressed(
        r#"{"action":"open_file","path":"C:/work/a.txt"}"#
    )));
}
