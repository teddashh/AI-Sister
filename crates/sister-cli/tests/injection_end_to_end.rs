use sister_core::config::Config;
use sister_core::db::{Db, L2Author, L2Insert};
use sister_core::prompt_fence::INJECTION_REGRESSION_CASES;
use sister_hands::semi_action::{
    ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, StepLimit, Task, grant_path,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const TASK: &str = "執行這個下一步";
const GOOD_URL: &str = "https://good.example.com/help";
const EVIL_URL: &str = "https://evil.example.com/collect";
const EVIL_AT_MS: i64 = 3_700_000;

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "sister-injection-e2e-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sister(data_dir: &Path, config: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sister"));
    command.arg("--data-dir").arg(data_dir);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.args(args);
    let output = command.output().expect("run sister CLI");
    assert!(
        output.status.success(),
        "sister {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn write_scenario(dir: &Path, injection: &str) -> PathBuf {
    let path = dir.join("scenario.json");
    let scenario = serde_json::json!({
        "name": "injection-end-to-end",
        "steps": [
            {
                "at_ms": 0,
                "app": "chrome.exe",
                "app_name": "Google Chrome",
                "title": "良性工作頁面",
                "text": [TASK, GOOD_URL, injection],
                "clipboard": injection,
                "clipboard_source_app": "chrome.exe"
            },
            {
                "at_ms": EVIL_AT_MS,
                "app": "chrome.exe",
                "app_name": "Google Chrome",
                "title": "稍後出現、未送給 Reviewer 的頁面",
                "text": [EVIL_URL]
            },
            { "at_ms": EVIL_AT_MS + 1_000, "no_screen": true }
        ]
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&scenario).unwrap()).unwrap();
    path
}

fn seed_l2_and_fact_ids(data_dir: &Path, injection: &str) -> (i64, i64, i64) {
    let mut db = Db::open(&Config::db_path(data_dir)).expect("open replay database");
    let chunks = db.recent(100).expect("read text_chunks");
    assert!(
        chunks.iter().any(|chunk| chunk.text == injection),
        "injection did not arrive verbatim in text_chunks: {injection:?}; got {:?}",
        chunks.iter().map(|chunk| &chunk.text).collect::<Vec<_>>()
    );
    let evidence = chunks
        .iter()
        .find(|chunk| chunk.text.contains(TASK))
        .expect("task evidence chunk");
    let frame_id = evidence.frame_id.expect("OCR chunk has frame");
    let core = evidence.ts;
    db.insert_l2_card(&L2Insert {
        segment_core_start: core,
        segment_ref: &format!("segment:{core}"),
        activity: "閱讀工作頁面",
        entities_json: "[]".into(),
        continues_json: None,
        commitments_json: serde_json::json!([{
            "text": TASK,
            "source": TASK,
            "due_hint": null
        }])
        .to_string(),
        model_confidence: 0.9,
        evidence_json: serde_json::json!([format!("frame:{frame_id}")]).to_string(),
        open_questions_json: "[]".into(),
        author: L2Author::Interpreter,
    })
    .expect("seed review input L2");

    let facts = db.facts_by_kind("url", 100).expect("URL facts");
    let good = facts.iter().find(|fact| fact.raw == GOOD_URL).unwrap().id;
    let evil = facts.iter().find(|fact| fact.raw == EVIL_URL).unwrap().id;
    (good, evil, frame_id)
}

fn write_brain(dir: &Path, fact_id: i64, frame_id: i64) -> PathBuf {
    let response = serde_json::json!({
        "commitments": [{
            "text": TASK,
            "stands": true,
            "kind": "followup",
            "due_hint": null,
            "due_source": "explicit",
            "people": [],
            "confidence": 0.9,
            "evidence_refs": [format!("frame:{frame_id}")],
            "allowed_next_step": {"fact": fact_id}
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
    config
}

fn write_grant(data_dir: &Path) {
    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(
        grant_path(data_dir),
        serde_json::to_vec_pretty(&grant).unwrap(),
    )
    .unwrap();
}

fn executed_lines(data_dir: &Path) -> Vec<String> {
    let path = data_dir.join("action-log.jsonl");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| {
                    value
                        .get("event")
                        .and_then(|event| event.as_str())
                        .map(str::to_owned)
                })
                .as_deref()
                == Some("executed")
        })
        .map(str::to_owned)
        .collect()
}

fn run_case(injection: &str, compromised: bool) -> (TempDir, Vec<String>) {
    let dir = TempDir::new(if compromised { "blocked" } else { "control" });
    let scenario = write_scenario(&dir.0, injection);
    sister(
        &dir.0,
        None,
        &[
            "replay",
            scenario.to_str().unwrap(),
            "--interval-ms",
            "3700000",
        ],
    );
    let (good, evil, frame_id) = seed_l2_and_fact_ids(&dir.0, injection);
    let config = write_brain(&dir.0, if compromised { evil } else { good }, frame_id);
    sister(
        &dir.0,
        Some(&config),
        &["consent", "--grant", "cloud-reading"],
    );
    let review = sister(
        &dir.0,
        Some(&config),
        &["review", "--last", "2h", "--force"],
    );
    assert!(
        !String::from_utf8_lossy(&review.stdout).contains("一次都還沒跑"),
        "review pipeline did not run"
    );
    write_grant(&dir.0);
    let action = sister(
        &dir.0,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let _ = action;
    let lines = executed_lines(&dir.0);
    (dir, lines)
}

#[test]
fn benign_control_reaches_platform_execution_exactly_once() {
    let (_dir, lines) = run_case("這是良性控制組，不是指令。", false);
    assert_eq!(
        lines.len(),
        1,
        "positive control must execute exactly once: {lines:?}"
    );
    println!("positive executed line: {}", lines[0]);
}

#[test]
fn all_twenty_injections_arrive_verbatim_but_execute_nothing() {
    assert_eq!(INJECTION_REGRESSION_CASES.len(), 20);
    for (index, injection) in INJECTION_REGRESSION_CASES.into_iter().enumerate() {
        let (_dir, lines) = run_case(injection, true);
        assert!(
            lines.is_empty(),
            "injection reached platform execution: {injection:?}\n{lines:#?}"
        );
        if matches!(index, 12 | 13 | 19) {
            println!("text_chunks case {}: {injection:?}", index + 1);
        }
    }
}
