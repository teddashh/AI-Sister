use sister_hands::semi_action::{
    ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, RunConclusionRecord, ScreenField,
    StepEvidence, StepLimit, TargetOnScreen, Task,
};
use sister_hands::{ActionEvent, ActionLog, ActionSnapshot, ApprovedBy, ExecutionResult};
use std::process::Command;

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sister-takeover-audit-{}-{}",
        std::process::id(),
        sister_core::now_ms()
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
