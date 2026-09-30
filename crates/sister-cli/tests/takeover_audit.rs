use sister_hands::semi_action::{
    ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, RunConclusionRecord, ScreenField,
    SemiActionRun, StepEvidence, StepLimit, StepRequest, TargetOnScreen, Task,
    execute_approved_step,
};
use sister_hands::{
    ActionEvent, ActionLog, ActionSnapshot, ApprovedBy, Attached, ExecutionResult, Executor,
    ExecutorError, Outcome, Suggestion, SuggestionButton,
    url_policy::{UrlOpenAnswer, UrlOpenPolicy, UrlOrigin},
};
use std::convert::Infallible;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Default)]
struct RecordingExecutor(Vec<ActionSnapshot>);

impl Executor for RecordingExecutor {
    fn execute(&mut self, suggestion: &Suggestion) -> Result<String, ExecutorError> {
        self.0.push(suggestion.snapshot());
        Ok("simulated OS result".into())
    }

    fn hands_attached(&self) -> Attached {
        Attached::Yes
    }
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sister-takeover-audit-{}-{}-{}",
        std::process::id(),
        sister_core::now_ms(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn hands_runs_json_is_a_complete_machine_readable_takeover_report() {
    let dir = temp_dir();
    let grant = Grant::new(
        Task::new("推 milestone"),
        AllowedApps::new([App::new("terminal.exe")]),
        AllowedActions::new([ActionKind::FocusWindow]),
        Expiry::after_issued(1_000, 60_000),
        StepLimit::new(2).unwrap(),
    );
    let grant_id = grant.audit_id();
    let action = ActionSnapshot::FocusWindow {
        title: "測試結果".into(),
    };
    let log = ActionLog::in_data_dir(&dir);
    for event in [
        ActionEvent::Granted {
            at_ms: 1_000,
            grant,
            run_id: Some("run-sha256:integration".into()),
            grant_id: Some(grant_id.clone()),
        },
        ActionEvent::Proposed {
            at_ms: 1_100,
            action: action.clone(),
        },
        ActionEvent::Approved {
            at_ms: 1_200,
            action: action.clone(),
            by: Some(ApprovedBy::StandingGrant),
        },
        ActionEvent::Executed {
            at_ms: 1_300,
            action: action.clone(),
            result: ExecutionResult::Succeeded {
                detail: "focused".into(),
            },
        },
        ActionEvent::StepFinished {
            at_ms: 1_500,
            step_number: 1,
            action,
            evidence: Some(StepEvidence::After {
                frame_id: 9,
                frame_at_ms: 1_450,
                has_image: true,
                waited_ms: 150,
                target: TargetOnScreen::Matched {
                    field: ScreenField::WindowTitle,
                    saw: "測試結果 — terminal".into(),
                    wanted: "測試結果".into(),
                },
            }),
        },
        ActionEvent::Concluded {
            at_ms: 1_900,
            conclusion: RunConclusionRecord::Completed {
                asked: Some(1),
                decided_by: Some(ApprovedBy::StandingGrant),
            },
        },
    ] {
        log.append(&event).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_sister"))
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "hands",
            "runs",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["hidden_earlier_runs"], 0);
    assert_eq!(value["unreadable"].as_array().unwrap().len(), 0);
    let report = &value["runs"][0];
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["run_id"], "run-sha256:integration");
    assert_eq!(report["grant_id"], grant_id);
    assert_eq!(report["started_at_ms"], 1_000);
    assert_eq!(report["ended_at_ms"], 1_900);
    assert_eq!(report["duration_ms"], 900);
    assert_eq!(report["complete"], true);
    assert_eq!(report["summary"]["approved_by_grant"], 1);
    assert_eq!(report["summary"]["screen_matched"], 1);
    assert_eq!(
        report["events"][4]["evidence"]["target"]["target_on_screen"],
        "matched"
    );
    assert_eq!(report["events"][4]["action"]["title"], "測試結果");

    let _ = std::fs::remove_dir_all(dir);
}

/// The ten tasks go through the public grant, approval, executor and persisted audit
/// path. The executor is simulated: this proves the contract and replay, not that a
/// Windows shell opened ten real targets or that Ted completed ten personal tasks.
#[test]
fn ten_scoped_tasks_are_executed_and_replayed_as_ten_distinct_runs() {
    let dir = temp_dir();
    let log = ActionLog::in_data_dir(&dir);
    let mut executor = RecordingExecutor::default();
    let actions: Vec<ActionSnapshot> = (0..10)
        .map(|index| match index % 3 {
            0 => ActionSnapshot::OpenFile {
                path: format!("C:/work/report-{index}.txt").into(),
            },
            1 => ActionSnapshot::FocusWindow {
                title: format!("Milestone {index}"),
            },
            _ => ActionSnapshot::OpenUrl {
                url: format!("https://example.test/report/{index}"),
            },
        })
        .collect();

    for (index, action) in actions.iter().enumerate() {
        let started = 10_000 + index as i64 * 1_000;
        let task = Task::new(format!("task-{index}"));
        let app = App::new("terminal.exe");
        let grant = Grant::new(
            task.clone(),
            AllowedApps::new([app.clone()]),
            AllowedActions::new([ActionKind::of(action)]),
            Expiry::after_issued(started, 500),
            StepLimit::new(1).unwrap(),
        );
        let grant_id = grant.audit_id();
        let step = StepRequest::new(task, app, action.clone());
        let url_policy = if matches!(action, ActionSnapshot::OpenUrl { .. }) {
            UrlOpenPolicy::Answered(UrlOpenAnswer::WhenYouCanNameTheOrigin)
        } else {
            UrlOpenPolicy::NotAskedYet
        };
        let (approval, permit) = grant
            .authorize_unattended(&step, started + 10, url_policy, |_| {
                Ok::<_, Infallible>(UrlOrigin::InHerRecord)
            })
            .expect("each task must mint its own scoped approval");
        let suggestion = SuggestionButton::parse_json(&serde_json::to_string(action).unwrap())
            .unwrap()
            .take_up(permit)
            .unwrap();
        let mut run = SemiActionRun::new(grant.clone());
        log.append(&ActionEvent::Granted {
            at_ms: started,
            grant,
            run_id: Some(format!("run-sha256:fixture-{index}")),
            grant_id: Some(grant_id),
        })
        .unwrap();
        log.append(&ActionEvent::Proposed {
            at_ms: started + 5,
            action: action.clone(),
        })
        .unwrap();
        let outcome = execute_approved_step(
            run.grant(),
            started + 10,
            approval,
            &step,
            &mut executor,
            &suggestion,
        );
        let Outcome::Done { detail } = outcome else {
            panic!("task {index} was not executed: {outcome:?}");
        };
        log.append(&ActionEvent::Approved {
            at_ms: started + 10,
            action: action.clone(),
            by: Some(ApprovedBy::StandingGrant),
        })
        .unwrap();
        log.append(&ActionEvent::Executed {
            at_ms: started + 20,
            action: action.clone(),
            result: ExecutionResult::Succeeded { detail },
        })
        .unwrap();
        log.append(&run.finish_step(started + 30, action.clone(), None).unwrap())
            .unwrap();
        assert!(
            run.may_start_step().is_err(),
            "task {index} exceeded one step"
        );
        log.append(&ActionEvent::Concluded {
            at_ms: started + 40,
            conclusion: RunConclusionRecord::Completed {
                asked: Some(1),
                decided_by: Some(ApprovedBy::StandingGrant),
            },
        })
        .unwrap();
    }

    assert_eq!(
        executor.0, actions,
        "the executor must see each exact target once"
    );
    let replay = log.replay().unwrap();
    assert!(replay.unreadable.is_empty());
    assert_eq!(replay.events.len(), 60);

    let output = Command::new(env!("CARGO_BIN_EXE_sister"))
        .args([
            "--data-dir",
            dir.to_str().unwrap(),
            "hands",
            "runs",
            "--json",
            "--limit",
            "10",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let audit: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(audit["hidden_earlier_runs"], 0);
    assert_eq!(audit["unreadable"].as_array().unwrap().len(), 0);
    let runs = audit["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 10);
    for (index, run) in runs.iter().enumerate() {
        assert_eq!(run["run_id"], format!("run-sha256:fixture-{index}"));
        assert_eq!(run["complete"], true);
        assert_eq!(run["summary"]["steps"], 1);
        assert_eq!(run["summary"]["approved_by_grant"], 1);
        assert_eq!(run["summary"]["succeeded"], 1);
        assert_eq!(
            run["events"][3]["action"],
            serde_json::to_value(&actions[index]).unwrap()
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
