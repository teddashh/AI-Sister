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
    run_case_ex(label, target_app, cite_target_frame, false)
}

fn run_case_ex(
    label: &str,
    target_app: &str,
    cite_target_frame: bool,
    card_cites_target: bool,
) -> (PathBuf, String, usize) {
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
                },
                // 目標之後還要有一格：目標那一刻如果是 `stream_end`，它就正好落在
                // 自己那一段的 `core_ended_at` 上，而 `facts_in_range` 是半開的，
                // 會把它排除在自己那一段之外。
                {
                    "at_ms": 120_000,
                    "app": target_app,
                    "app_name": "目標所在 app",
                    "title": "下一步目標",
                    "text": ["他繼續在同一個視窗做事"]
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
    // 卡片掛在**目標所在的那一段**。#48 之後審閱者只看卡片自己那一段，而換 app
    // 就會切段——這幾條測試要驗的是「授權涵不涵蓋目標那個 app」，不是跨段指標，
    // 所以夾具要把卡片放在目標那一段，才問得到後面那個問題。
    let target_chunk = chunks
        .iter()
        .find(|c| c.text.contains(OTHER_APP_URL))
        .expect("target chunk");
    let last_ts = chunks.iter().map(|c| c.ts).max().unwrap_or(evidence.ts);
    let segs = db
        .chapters_for_range(evidence.ts, last_ts.saturating_add(1_000))
        .expect("compute segments");
    let core = segs
        .iter()
        .find(|s| s.core_started_at <= target_chunk.ts && target_chunk.ts < s.core_ended_at)
        .unwrap_or_else(|| {
            panic!(
                "no segment covering the target frame at {}; got {:?}",
                target_chunk.ts,
                segs.iter()
                    .map(|s| (s.core_started_at, s.core_ended_at))
                    .collect::<Vec<_>>()
            )
        })
        .core_started_at;
    let target_frame_for_card = db
        .facts_by_kind("url", 100)
        .unwrap()
        .into_iter()
        .find(|f| f.raw == OTHER_APP_URL)
        .and_then(|f| f.frame_id);
    let card_evidence_json = if card_cites_target {
        let tf = target_frame_for_card.expect("target frame for card");
        serde_json::json!([format!("frame:{frame_id}"), format!("frame:{tf}")]).to_string()
    } else {
        serde_json::json!([format!("frame:{frame_id}")]).to_string()
    };
    db.insert_l2_card(&L2Insert {
        segment_core_start: core,
        segment_ref: &format!("segment:{core}"),
        activity: "閱讀工作頁面",
        entities_json: "[]".into(),
        commitments_json: serde_json::json!([{ "text": TASK, "source": TASK, "due_hint": null }])
            .to_string(),
        continues_json: None,
        model_confidence: 0.9,
        evidence_json: card_evidence_json.clone(),
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
fn target_frame_never_shown_to_the_model_is_dropped_and_unattended_refuses() {
    let (dir, stdout, executed) = run_case("same-app-sprayed-frame", "chrome.exe", true);
    assert_eq!(executed, 0, "stdout:\n{stdout}");
    assert!(
        stdout.contains("承諾沒有引用下一步目標"),
        "stdout:\n{stdout}"
    );
    let db = Db::open(&Config::db_path(&dir)).expect("open reviewed database");
    let commitment = db.live_commitments().unwrap().pop().expect("commitment");
    let target_fact = db
        .fact_by_id(
            commitment
                .allowed_next_step_fact
                .expect("reviewed next-step fact"),
        )
        .unwrap()
        .expect("target fact");
    let target_ref = format!("frame:{}", target_fact.frame_id.expect("target frame"));
    let evidence_refs: Vec<String> = serde_json::from_str(&commitment.evidence_json).unwrap();
    assert!(
        !evidence_refs.contains(&target_ref),
        "reviewer 沒展示的目標 frame 不得留在 evidence_json：{evidence_refs:?}"
    );
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).expect("action log");
    assert!(
        log.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|event| event["event"] == "refused"
                && event["reason"]["refusal"] == "unattended_target_has_no_cited_frame"
                && event["reason"]["why"] == "frame_not_cited"),
        "action log:\n{log}"
    );
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

/// 誠實引用的那條路**還走得通**——這是「乾脆全部拒絕」的偵測器。
///
/// #49 之後，無人值守要成立需要兩件事同時為真：承諾引用了目標所在的那張畫面
/// （alpha.84），而且那張畫面**真的有拿給模型看過**（這一版）。所以卡片自己
/// 引用了目標那張 frame 的時候，這一步照樣會做出去。
///
/// **這一條不准刪。** 沒有它的話，把 `evidence_refs` 一律清空也會全綠。
/// （codex 交這一版的時候刪掉的正是它的前身
/// `target_on_a_cited_frame_executes_unattended`。）
#[test]
fn target_on_a_frame_the_card_cited_still_executes_unattended() {
    let (_, stdout, executed) = run_case_ex("card-cites-target", "chrome.exe", true, true);
    assert_eq!(executed, 1, "stdout:\n{stdout}");
    assert!(!stdout.contains("無人值守拒絕"), "stdout:\n{stdout}");
}

fn run_agreed_unattended(label: &str, pass_b_cites_target: bool) -> (PathBuf, String, usize) {
    let dir = tmp(label);
    let scenario = dir.join("scenario.json");
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "agreed-evidence-probe",
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
                    "app": "chrome.exe",
                    "app_name": "Google Chrome",
                    "title": "下一步目標",
                    "text": [OTHER_APP_URL]
                },
                {
                    "at_ms": 120_000,
                    "app": "chrome.exe",
                    "app_name": "Google Chrome",
                    "title": "下一步目標",
                    "text": ["他繼續在同一個視窗做事"]
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
    let work_frame = evidence.frame_id.expect("work frame");
    let target_chunk = chunks
        .iter()
        .find(|c| c.text.contains(OTHER_APP_URL))
        .expect("target chunk");
    let last_ts = chunks.iter().map(|c| c.ts).max().unwrap_or(evidence.ts);
    let segs = db
        .chapters_for_range(evidence.ts, last_ts.saturating_add(1_000))
        .expect("compute segments");
    let core = segs
        .iter()
        .find(|s| s.core_started_at <= target_chunk.ts && target_chunk.ts < s.core_ended_at)
        .unwrap_or_else(|| {
            panic!(
                "no segment covering the target frame at {}",
                target_chunk.ts
            )
        })
        .core_started_at;
    let target = db
        .facts_by_kind("url", 100)
        .unwrap()
        .into_iter()
        .find(|f| f.raw == OTHER_APP_URL)
        .expect("target fact");
    let target_frame = target.frame_id.expect("target frame");
    db.insert_l2_card(&L2Insert {
        segment_core_start: core,
        segment_ref: &format!("segment:{core}"),
        activity: "閱讀工作頁面",
        entities_json: "[]".into(),
        commitments_json: serde_json::json!([{ "text": TASK, "source": TASK, "due_hint": null }])
            .to_string(),
        continues_json: None,
        model_confidence: 0.9,
        evidence_json: serde_json::json!([
            format!("frame:{work_frame}"),
            format!("frame:{target_frame}")
        ])
        .to_string(),
        open_questions_json: "[]".into(),
        author: L2Author::Interpreter,
    })
    .unwrap();

    let commitment_body = |refs: Vec<String>| {
        serde_json::json!({
            "commitments": [{
                "text": TASK,
                "stands": true,
                "kind": "followup",
                "due_hint": null,
                "due_source": "explicit",
                "people": [],
                "confidence": 0.9,
                "evidence_refs": refs,
                "allowed_next_step": {"fact": target.id}
            }]
        })
        .to_string()
    };
    let json_a = commitment_body(vec![format!("frame:{target_frame}")]);
    let json_b = if pass_b_cites_target {
        json_a.clone()
    } else {
        commitment_body(vec![format!("frame:{work_frame}")])
    };
    let script = dir.join("fake-brain.py");
    std::fs::write(
        &script,
        format!(
            "import sys\npayload = sys.stdin.buffer.read()\nbody = {json_b:?}.encode('utf-8') if b'PASS_B' in payload else {json_a:?}.encode('utf-8')\nsys.stdout.buffer.write(body)\n"
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
    sister(&dir, Some(&config), &["review", "--last", "2h", "--force"]);

    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();
    let action = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let stdout = String::from_utf8(action.stdout).unwrap();
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).unwrap_or_default();
    let executed = log
        .lines()
        .filter(|line| line.contains("\"event\":\"executed\""))
        .count();
    (dir, stdout, executed)
}

#[test]
fn unattended_proceeds_when_both_passes_cited_the_target_frame() {
    let (_, stdout, executed) = run_agreed_unattended("agreed-both", true);
    assert_eq!(executed, 1, "stdout:\n{stdout}");
    assert!(!stdout.contains("無人值守拒絕"), "stdout:\n{stdout}");
}

#[test]
fn unattended_refuses_when_only_one_pass_cited_the_target_frame() {
    let (dir, stdout, executed) = run_agreed_unattended("agreed-one-pass", false);
    assert_eq!(executed, 0, "stdout:\n{stdout}");
    assert!(stdout.contains("只有一個 pass 指過"), "stdout:\n{stdout}");
    let db = Db::open(&Config::db_path(&dir)).unwrap();
    let commitment = db.live_commitments().unwrap().pop().expect("commitment");
    let target_fact = db
        .fact_by_id(
            commitment
                .allowed_next_step_fact
                .expect("reviewed next-step fact"),
        )
        .unwrap()
        .expect("target fact");
    let target_ref = format!("frame:{}", target_fact.frame_id.expect("target frame"));
    let union: Vec<String> = serde_json::from_str(&commitment.evidence_json).unwrap();
    assert!(
        union.contains(&target_ref),
        "同一份資料在舊行為下讀聯集會放行：{union:?}"
    );
    let agreed = match commitment.agreed_evidence_json.as_deref() {
        None => panic!("新寫入的承諾不該是 NULL"),
        Some(json) => serde_json::from_str::<Vec<String>>(json).unwrap(),
    };
    assert!(
        !agreed.contains(&target_ref),
        "交集不該含只有一個 pass 指過的畫面：{agreed:?}"
    );
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).expect("action log");
    assert!(
        log.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|event| event["event"] == "refused"
                && event["reason"]["refusal"] == "unattended_target_has_no_cited_frame"
                && event["reason"]["why"] == "cited_by_only_one_pass"),
        "action log:\n{log}"
    );
}

#[test]
fn unattended_refuses_old_commitment_recorded_before_agreed_evidence() {
    let (dir, _, _) = run_agreed_unattended("agreed-null-old", true);
    {
        let conn = rusqlite::Connection::open(Config::db_path(&dir)).unwrap();
        conn.execute_batch(
            "ALTER TABLE commitments DROP COLUMN agreed_evidence_json;
             PRAGMA user_version = 14;",
        )
        .unwrap();
    }
    let db = Db::open(&Config::db_path(&dir)).unwrap();
    let commitment = db.live_commitments().unwrap().pop().expect("commitment");
    assert_eq!(commitment.agreed_evidence_json, None);
    drop(db);

    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();
    let _ = std::fs::remove_file(dir.join("action-log.jsonl"));
    let action = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let stdout = String::from_utf8(action.stdout).unwrap();
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).unwrap_or_default();
    let executed = log
        .lines()
        .filter(|line| line.contains("\"event\":\"executed\""))
        .count();
    assert_eq!(executed, 0, "stdout:\n{stdout}");
    assert!(stdout.contains("記在加這道檢查之前"), "stdout:\n{stdout}");
    assert!(
        stdout.contains("問不出兩個 pass 同不同意"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("只有一個 pass 指過"), "stdout:\n{stdout}");
    assert!(
        log.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|event| event["event"] == "refused"
                && event["reason"]["why"] == "recorded_before_agreed_evidence"),
        "action log:\n{log}"
    );
}

#[test]
fn unattended_empty_agreed_evidence_is_not_the_null_sentence() {
    let (dir, _, _) = run_agreed_unattended("agreed-empty", true);
    {
        let conn = rusqlite::Connection::open(Config::db_path(&dir)).unwrap();
        // 兩欄都清掉。**只清交集是到不了這一格的**：聯集還留著目標那張畫面時，
        // 拒絕的理由會是「只有一個 pass 指過」。這件事本身就是「交集是空的」當不
        // 成獨立拒絕理由的證明——要造出那個狀態，得先讓聯集也空掉，而那時候誰都
        // 沒指過那張畫面，該講的就是這一句。
        conn.execute(
            "UPDATE commitments SET agreed_evidence_json = '[]', evidence_json = '[]'",
            [],
        )
        .unwrap();
    }
    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();
    let _ = std::fs::remove_file(dir.join("action-log.jsonl"));
    let action = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let stdout = String::from_utf8(action.stdout).unwrap();
    assert!(
        stdout.contains("承諾沒有引用下一步目標"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("記在加這道檢查之前"), "stdout:\n{stdout}");
    assert!(!stdout.contains("只有一個 pass 指過"), "stdout:\n{stdout}");
    let log = std::fs::read_to_string(dir.join("action-log.jsonl")).unwrap_or_default();
    assert!(
        log.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|event| event["event"] == "refused"
                && event["reason"]["why"] == "frame_not_cited"),
        "action log:\n{log}"
    );
}
