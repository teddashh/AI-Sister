//! 下一步目標自己的畫面來源必須參與授權書的 app 維度。
use sister_core::config::Config;
use sister_core::db::{Db, L2Author, L2Insert};
use sister_hands::semi_action::{
    ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, StepLimit, Task, grant_path,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TASK: &str = "執行這個下一步";
const OTHER_APP_URL: &str = "https://from-another-app.example.com/collect";

fn tmp(label: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("sister-probe-prov-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn sister(data_dir: &Path, config: Option<&Path>, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sister"));
    c.arg("--data-dir").arg(data_dir);
    if let Some(config) = config {
        c.arg("--config").arg(config);
    }
    c.args(args);
    let out = c.output().expect("run sister");
    assert!(
        out.status.success(),
        "sister {args:?} failed\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn run_case(label: &str, target_app: &str, cite_target_frame: bool) -> (PathBuf, String, usize) {
    let dir = tmp(label);
    let scenario = dir.join("scenario.json");
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "app-provenance-probe",
            "steps": [
                {
                    "at_ms": 0,
                    "app": "chrome.exe",
                    "app_name": "Google Chrome",
                    "title": "他真的在工作的那個視窗",
                    "text": [TASK]
                },
                {
                    "at_ms": 60_000,
                    "app": target_app,
                    "app_name": "目標所在 app",
                    "title": "下一步目標",
                    "text": [OTHER_APP_URL]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    sister(
        &dir,
        None,
        &[
            "replay",
            scenario.to_str().unwrap(),
            "--interval-ms",
            "60000",
        ],
    );

    let mut db = Db::open(&Config::db_path(&dir)).unwrap();
    let chunks = db.recent(100).unwrap();
    let evidence = chunks
        .iter()
        .find(|c| c.text.contains(TASK))
        .expect("task chunk");
    let frame_id = evidence.frame_id.expect("frame");
    let core = evidence.ts;
    db.insert_l2_card(&L2Insert {
        segment_core_start: core,
        segment_ref: &format!("segment:{core}"),
        activity: "閱讀工作頁面",
        entities_json: "[]".into(),
        commitments_json: serde_json::json!([{ "text": TASK, "source": TASK, "due_hint": null }])
            .to_string(),
        continues_json: None,
        model_confidence: 0.9,
        evidence_json: serde_json::json!([format!("frame:{frame_id}")]).to_string(),
        open_questions_json: "[]".into(),
        author: L2Author::Interpreter,
    })
    .unwrap();

    let facts = db.facts_by_kind("url", 100).unwrap();
    eprintln!(
        ">>> url facts: {:?}",
        facts.iter().map(|f| (f.id, &f.raw)).collect::<Vec<_>>()
    );
    let target = facts
        .iter()
        .find(|f| f.raw == OTHER_APP_URL)
        .expect("slack url became a fact");

    // 假大腦：承諾的證據指 chrome 的 frame，下一步指另一格 fact。
    let mut evidence_refs = vec![format!("frame:{frame_id}")];
    if cite_target_frame {
        evidence_refs.push(format!("frame:{}", target.frame_id.expect("target frame")));
    }
    let response = serde_json::json!({
        "commitments": [{
            "text": TASK,
            "stands": true,
            "kind": "followup",
            "due_hint": null,
            "due_source": "explicit",
            "people": [],
            "confidence": 0.9,
            "evidence_refs": evidence_refs,
            "allowed_next_step": {"fact": target.id}
        }]
    })
    .to_string();
    let script = dir.join("fake-brain.py");
    std::fs::write(
        &script,
        format!(
            "import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write({response:?}.encode('utf-8'))\n"
        ),
    )
    .unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[brain]\ncommand = \"python3\"\nargs = [{}]\nreviewer_daily_budget = 40\n",
            serde_json::to_string(&script.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();

    sister(
        &dir,
        Some(&config),
        &["consent", "--grant", "cloud-reading"],
    );
    let review = sister(&dir, Some(&config), &["review", "--last", "2h", "--force"]);
    eprintln!(
        ">>> review stdout:\n{}",
        String::from_utf8_lossy(&review.stdout)
    );

    // 授權書只列 chrome.exe。
    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();

    // 這個整合 helper 只能驗 unattended：attended 在 stdin 不是 TTY 時會先被
    // `who_answers` 拒絕。正向控制在 ops.rs 的單元測試用 live press 接縫驗。
    let action = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    eprintln!(
        ">>> do stdout:\n{}",
        String::from_utf8_lossy(&action.stdout)
    );
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).unwrap_or_default();
    let executed = log
        .lines()
        .filter(|l| l.contains("\"event\":\"executed\""))
        .count();
    eprintln!(">>> executed lines = {executed}");
    (dir, String::from_utf8(action.stdout).unwrap(), executed)
}

#[test]
fn target_from_another_app_is_not_covered() {
    let (_, stdout, executed) = run_case("two-apps", "slack.exe", false);
    assert_eq!(executed, 0, "stdout:\n{stdout}");
    assert!(stdout.contains("授權不涵蓋"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("這個目標是在 slack.exe 的畫面上看到的"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn target_from_same_app_is_still_refused_unattended_when_not_cited() {
    let (dir, stdout, executed) = run_case("same-app-unattended", "chrome.exe", false);
    assert_eq!(executed, 0, "stdout:\n{stdout}");
    assert!(stdout.contains("授權涵蓋這一步"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("承諾沒有引用下一步目標"),
        "stdout:\n{stdout}"
    );
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).expect("action log");
    let refused: Vec<serde_json::Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|event: &serde_json::Value| event["event"] == "refused")
        .collect();
    assert!(
        refused.iter().any(|event| {
            event["reason"]["refusal"] == "unattended_target_has_no_cited_frame"
                && event["reason"]["why"] == "frame_not_cited"
        }),
        "action log:\n{log}"
    );
    assert!(!log.contains("not_covered_by_grant"), "action log:\n{log}");
    assert!(
        !log.contains("user_declined_this_step"),
        "action log:\n{log}"
    );
}

#[test]
fn target_on_a_cited_frame_executes_unattended() {
    let (_, stdout, executed) = run_case("same-app-cited", "chrome.exe", true);
    assert_eq!(executed, 1, "stdout:\n{stdout}");
    assert!(!stdout.contains("無人值守拒絕"), "stdout:\n{stdout}");
}

#[test]
fn schema_13_commitment_is_refused_with_missing_target_provenance() {
    let (dir, _, _) = run_case("schema-13", "chrome.exe", true);
    {
        let conn = rusqlite::Connection::open(Config::db_path(&dir)).unwrap();
        conn.execute_batch(
            "ALTER TABLE commitments DROP COLUMN allowed_next_step_fact;
             PRAGMA user_version = 13;",
        )
        .unwrap();
    }

    let db = Db::open(&Config::db_path(&dir)).unwrap();
    let commitment = db.live_commitments().unwrap().pop().expect("commitment");
    assert_eq!(commitment.allowed_next_step_fact, None);
    drop(db);

    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();
    std::fs::remove_file(dir.join("action-log.jsonl")).unwrap();
    let action = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let stdout = String::from_utf8(action.stdout).unwrap();
    eprintln!(">>> upgraded do stdout:\n{stdout}");
    assert!(
        stdout.contains("這個目標是從哪個畫面來的沒有記"),
        "stdout:\n{stdout}"
    );
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).unwrap_or_default();
    let executed = log
        .lines()
        .filter(|line| line.contains("\"event\":\"executed\""))
        .count();
    assert_eq!(executed, 0, "stdout:\n{stdout}");
    assert!(
        stdout.contains("沒有記下一步目標的 fact 出處"),
        "stdout:\n{stdout}"
    );
}
