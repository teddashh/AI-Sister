use sister_core::config::Config;
use sister_core::db::{Db, L2Author, L2Insert};
use sister_core::model::{FocusEvent, FocusKind, FocusSnapshot};
use sister_core::prompt_fence::INJECTION_REGRESSION_CASES;
use sister_hands::semi_action::{
    ActionKind, AllowedActions, AllowedApps, App, ApprovedCommitment, Expiry, Grant, StepLimit,
    Task, grant_path,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const TASK: &str = "執行這個下一步";
const GOOD_URL: &str = "https://good.example.com/help";
const EVIL_URL: &str = "https://evil.example.com/collect";
const INJECTED_URL: &str = "https://example.com/collect";
const BENIGN_PATH: &str = r"C:\work\report.txt";
const INJECTED_PATH: &str = r"C:\work\collect.txt";
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
        "privacy_context": "clear",
        "system_state": "active",
        "steps": [
            {
                "at_ms": 0,
                "app": "chrome.exe",
                "app_name": "Google Chrome",
                "title": "良性工作頁面",
                // 兩格都給網址，**兩個站都真的被瀏覽過**。這是刻意的：alpha.100
                // 那道網址來源閘門（#42）如果在這裡擋掉 evil，這一整套就會為了
                // 另一個理由變綠，而它原本要證的那件事（時間窗把 evil 關在外面）
                // 就沒有人在跑了。讓新閘門對兩邊都中立，這套測試的軸才沒有變。
                "url": GOOD_URL,
                "text": [TASK, "受控工作頁面", injection],
                "clipboard": injection,
                "clipboard_source_app": "chrome.exe"
            },
            {
                "at_ms": EVIL_AT_MS,
                "app": "chrome.exe",
                "app_name": "Google Chrome",
                "title": "稍後出現、未送給 Reviewer 的頁面",
                "url": EVIL_URL,
                "text": [EVIL_URL]
            },
            { "at_ms": EVIL_AT_MS + 1_000, "no_screen": true }
        ]
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&scenario).unwrap()).unwrap();
    path
}

fn seed_l2_and_fact_ids(data_dir: &Path, injection: &str, trusted_source: bool) -> (i64, i64, i64) {
    let mut db = Db::open(&Config::db_path(data_dir)).expect("open replay database");
    let chunks = db.recent(100).expect("read text_chunks");
    assert!(
        chunks.iter().any(|chunk| chunk.text.contains(injection)),
        "injection did not arrive verbatim in text_chunks: {injection:?}; got {:?}",
        chunks.iter().map(|chunk| &chunk.text).collect::<Vec<_>>()
    );
    let evidence = chunks
        .iter()
        .find(|chunk| chunk.text.contains(TASK))
        .expect("task evidence chunk");
    let frame_id = evidence.frame_id.expect("OCR chunk has frame");
    // 真的卡片是從真的 segment 生出來的。這套 fixture 以前直接種 L2、跳過
    // 解釋層，`segment` 表是空的；審閱者改成讀這一段的結束時間之後，空表
    // 會 fail-closed。
    //
    // 只蓋第一格畫面不夠：replay 下一拍在 EVIL_AT_MS，那之前只有一個時間點，
    // segmenter 在 `stream_end == stream_start` 時切不出段落。範圍拉到含第二
    // 格，讓它真的切得成；TIME_CAP 會把第一段收在 10 分鐘內，evil 仍在窗外。
    let segs = db
        .chapters_for_range(evidence.ts, evidence.ts.saturating_add(EVIL_AT_MS + 2_000))
        .expect("compute segment covering first frame");
    let seg = segs
        .iter()
        .find(|s| s.core_started_at <= evidence.ts && evidence.ts < s.core_ended_at)
        .unwrap_or_else(|| {
            panic!(
                "fixture must produce a segment covering the first frame at {}; got {:?}",
                evidence.ts,
                segs.iter()
                    .map(|s| (s.core_started_at, s.core_ended_at))
                    .collect::<Vec<_>>()
            )
        });
    let core = seg.core_started_at;
    let window_end = seg
        .core_ended_at
        .saturating_add(sister_core::segment::OVERLAP_MARGIN_MS);
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
    let good = facts
        .iter()
        .find(|fact| {
            fact.raw == GOOD_URL && fact.source_kind == "url" && fact.frame_id == Some(frame_id)
        })
        .or_else(|| facts.iter().find(|fact| fact.raw == GOOD_URL))
        .expect("replay must retain the good URL fact");
    let evil = facts.iter().find(|fact| fact.raw == EVIL_URL).unwrap();
    assert!(
        good.ts >= core && good.ts < window_end,
        "good URL must stay inside this segment's window: good.ts={} core={core} window_end={window_end}",
        good.ts
    );
    assert!(
        evil.ts >= window_end,
        "evil URL must stay outside this segment's window: evil.ts={} window_end={window_end}",
        evil.ts
    );
    // Replay 的 URL 不能替 unattended 動作背書；否則下載一份語料就能種票。
    // 這套測的是另一個軸，所以合成一場標成可信 Windows recorder 的測試
    // session，讓網址政策對 good / evil 都保持中立。
    let session = db
        .start_session(sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM, "test")
        .expect("synthetic trusted capture session");
    for (index, url) in [GOOD_URL, EVIL_URL].into_iter().enumerate() {
        db.insert_focus(
            session,
            &FocusEvent {
                ts: 10_000_000 + index as i64,
                kind: FocusKind::UrlChange,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    app_name: Some("Google Chrome".into()),
                    window_title: Some("網址來源夾具".into()),
                    url: Some(url.into()),
                    pid: Some(1),
                },
            },
        )
        .expect("seed synthetic trusted URL provenance");
    }
    if trusted_source {
        // Replay 本身永遠不會取得來源票。這個測試夾具明確把第一張畫面及其
        // OCR 紀錄改成合成的可信 recorder 輸出，讓原有時間窗測試仍能觀察到
        // 良性網址執行與晚到的惡意網址被時間窗拒絕。第二張保留 replay 身分。
        let conn = rusqlite::Connection::open(Config::db_path(data_dir)).unwrap();
        for (table, id_column) in [
            ("frames", "id"),
            ("text_chunks", "frame_id"),
            ("facts", "frame_id"),
        ] {
            conn.execute(
                &format!("UPDATE {table} SET session_id = ?1 WHERE {id_column} = ?2"),
                rusqlite::params![session, frame_id],
            )
            .unwrap();
        }
    }
    (good.id, evil.id, frame_id)
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
    write_brain_raw(dir, &response)
}

fn write_brain_raw(dir: &Path, response_json_text: &str) -> PathBuf {
    let script = dir.join("fake-brain.py");
    std::fs::write(
        &script,
        format!(
            "import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write({response_json_text:?}.encode('utf-8'))\n"
        ),
    )
    .unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[brain]\ncommand = \"python3\"\nargs = [{}]\nreviewer_daily_budget = 40\n\
             [hands]\nurl_open = \"when-you-can-name-the-origin\"\n",
            serde_json::to_string(&script.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    config
}

#[derive(Clone, Copy)]
enum GrantBinding {
    None,
    Live,
    ArchivedSibling,
}

fn write_grant(data_dir: &Path, allowed_url: &str, binding: GrantBinding) {
    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    )
    .with_url_targets([allowed_url.to_owned()])
    .unwrap();
    let grant = match binding {
        GrantBinding::None => grant,
        GrantBinding::Live | GrantBinding::ArchivedSibling => {
            let card = Db::open(&Config::db_path(data_dir))
                .unwrap()
                .live_commitments()
                .unwrap()
                .into_iter()
                .next()
                .expect("reviewed card");
            let action = match sister_hands::commitment_action::parse_allowed_next_step(
                card.allowed_next_step.as_deref(),
            ) {
                sister_hands::commitment_action::AllowedNextStep::Suggestion(button) => {
                    button.snapshot()
                }
                other => panic!("reviewed card must have a concrete action: {other:?}"),
            };
            let (id, text) = if matches!(binding, GrantBinding::ArchivedSibling) {
                // A previously selected card with the same address and action exists in
                // the controlled DB, but was archived before this run. The screen can
                // produce a *new* card; it cannot inherit that older selection.
                let conn = rusqlite::Connection::open(Config::db_path(data_dir)).unwrap();
                conn.execute(
                    "INSERT INTO commitments(
                        text, kind, born_from, evidence_json, agreed_evidence_json, people_json,
                        due_hint, due_source, due_at, status, confidence, allowed_next_step,
                        allowed_next_step_fact, last_evidence_seen_at, kill_note,
                        created_at, updated_at, tombstoned_at)
                     SELECT '先前核准的受控工作', kind, born_from, evidence_json, agreed_evidence_json,
                        people_json, due_hint, due_source, due_at, status, confidence,
                        allowed_next_step, allowed_next_step_fact, last_evidence_seen_at,
                        kill_note, created_at, updated_at, ?2
                       FROM commitments WHERE id = ?1",
                    rusqlite::params![card.id, sister_core::now_ms()],
                )
                .unwrap();
                (conn.last_insert_rowid(), "先前核准的受控工作".to_owned())
            } else {
                (card.id, card.text)
            };
            grant.with_approved_commitment(ApprovedCommitment {
                id,
                text,
                action,
                target_fact_id: card.allowed_next_step_fact,
                agreed_evidence_json: card.agreed_evidence_json,
            })
        }
    };
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
    let (good, evil, frame_id) = seed_l2_and_fact_ids(&dir.0, injection, true);
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
    write_grant(
        &dir.0,
        GOOD_URL,
        if compromised {
            GrantBinding::None
        } else {
            GrantBinding::Live
        },
    );
    // **`--config` 不能省。** 沒有它的話 `do` 讀的是 `Config::default_path()`
    // 那一份（開發機上真的那一份），資料目錄裡這一份完全不會被看到——
    // 於是 `[hands] url_open` 沒生效，整套停在「我還沒問過你」。
    let action = sister(
        &dir.0,
        Some(&config),
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let _ = action;
    let lines = executed_lines(&dir.0);
    (dir, lines)
}

fn run_raw_brain_case(response: impl FnOnce(i64) -> String) -> (TempDir, Vec<String>) {
    let injection = "這是偽造大腦回答的端到端測試。";
    let dir = TempDir::new("raw-brain");
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
    let (_good, _evil, frame_id) = seed_l2_and_fact_ids(&dir.0, injection, true);
    let config = write_brain_raw(&dir.0, &response(frame_id));
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
    write_grant(&dir.0, GOOD_URL, GrantBinding::None);
    let did = sister(
        &dir.0,
        Some(&config),
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let _ = did;
    let lines = executed_lines(&dir.0);
    (dir, lines)
}

fn commitment_response(frame_id: i64, allowed_next_step: serde_json::Value) -> String {
    serde_json::json!({
        "commitments": [{
            "text": TASK,
            "stands": true,
            "kind": "followup",
            "due_hint": null,
            "due_source": "explicit",
            "people": [],
            "confidence": 0.9,
            "evidence_refs": [format!("frame:{frame_id}")],
            "allowed_next_step": allowed_next_step
        }]
    })
    .to_string()
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
fn late_url_fact_remains_outside_the_review_window() {
    let (_dir, lines) = run_case(INJECTION_REGRESSION_CASES[0], true);
    assert!(
        lines.is_empty(),
        "late URL reached platform execution: {lines:#?}"
    );
}

fn injected_url_from_source(
    injection: &str,
    source_url: Option<&str>,
    trusted_source: bool,
    allowed_url: &str,
    target_from_address: bool,
    append_target_to_screen: bool,
    binding: GrantBinding,
) -> (Vec<String>, String) {
    let dir = TempDir::new("in-frame-url");
    let scenario = write_scenario(&dir.0, injection);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&scenario).unwrap()).unwrap();
    // Replay 沒有 URL 時會整張不收；「有畫面但位址列沒量到」要在寫入後
    // 合成，才真的測到來源判斷的那一格。
    value["steps"][0]["url"] = source_url.unwrap_or(GOOD_URL).into();
    // 攻擊場景仍保留一筆良性 URL fact 作為另一個可選目標；正向控制
    // 的頁面正文則不重複位址列，才能驗證乾淨的地址 fact 可執行。
    value["steps"][0]["text"][1] = GOOD_URL.into();
    // 每條語料都在本段、被引用的同一張畫面中提供可抽取 URL。第 8 條
    // 本身已有該 URL；其他條把它接在原文後面，保留原文逐字到達。
    let attack_text = if !append_target_to_screen || injection.contains(INJECTED_URL) {
        injection.to_owned()
    } else {
        format!("{injection}\n{INJECTED_URL}")
    };
    value["steps"][0]["text"][2] = attack_text.into();
    std::fs::write(&scenario, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
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
    let (good, _, frame_id) = seed_l2_and_fact_ids(&dir.0, injection, trusted_source);
    if source_url.is_none() {
        rusqlite::Connection::open(Config::db_path(&dir.0))
            .unwrap()
            .execute("UPDATE frames SET url = NULL WHERE id = ?1", [frame_id])
            .unwrap();
    }
    let injected = {
        let mut db = Db::open(&Config::db_path(&dir.0)).unwrap();
        let fact = db
            .facts_by_kind("url", 100)
            .unwrap()
            .into_iter()
            .find(|fact| {
                fact.frame_id == Some(frame_id)
                    && if target_from_address {
                        fact.source_kind == "url"
                            && sister_hands::target_policy::same_destination(
                                &fact.raw,
                                INJECTED_URL,
                            )
                    } else {
                        fact.raw == INJECTED_URL && fact.source_kind == "ocr"
                    }
            })
            .expect("injected URL must be extracted from the cited screen");
        let chunks = db.recent(100).unwrap();
        assert!(
            chunks.iter().any(|chunk| {
                Some(chunk.chunk_id) == fact.chunk_id
                    && if target_from_address {
                        chunk.source_kind == sister_core::model::SourceKind::Url
                    } else {
                        chunk.text.contains(injection) && chunk.text.contains(INJECTED_URL)
                    }
            }),
            "target fact must be present on the cited frame: {injection:?}"
        );
        let session = db
            .start_session(sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM, "test")
            .unwrap();
        db.insert_focus(
            session,
            &FocusEvent {
                ts: 10_000_100,
                kind: FocusKind::UrlChange,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    url: Some(INJECTED_URL.into()),
                    ..FocusSnapshot::default()
                },
            },
        )
        .unwrap();
        fact.id
    };
    assert_ne!(good, injected);
    let config = write_brain(&dir.0, injected, frame_id);
    sister(
        &dir.0,
        Some(&config),
        &["consent", "--grant", "cloud-reading"],
    );
    sister(
        &dir.0,
        Some(&config),
        &["review", "--last", "2h", "--force"],
    );
    let commitments = Db::open(&Config::db_path(&dir.0))
        .unwrap()
        .live_commitments()
        .unwrap();
    assert!(
        commitments
            .iter()
            .any(|commitment| commitment.allowed_next_step_fact == Some(injected)),
        "reviewer did not accept the in-window fact from this case: {injection:?}"
    );
    write_grant(&dir.0, allowed_url, binding);
    sister(
        &dir.0,
        Some(&config),
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let lines = executed_lines(&dir.0);
    let log = std::fs::read_to_string(dir.0.join("action-log.jsonl")).unwrap();
    (lines, log)
}

fn assert_url_refused(lines: &[String], log: &str, expected_reason: &str) {
    assert!(
        lines.is_empty(),
        "screen text borrowed a standing grant: {lines:#?}"
    );
    assert!(
        log.lines().any(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .is_some_and(|event| {
                    event["event"] == "refused"
                        && if expected_reason == "not_covered_by_grant" {
                            event["reason"]["refusal"] == expected_reason
                                && event["reason"]["rejection"] == "Target"
                        } else if expected_reason == "unapproved_commitment" {
                            event["reason"]["refusal"] == "not_covered_by_grant"
                                && event["reason"]["rejection"] == "Commitment"
                        } else {
                            event["reason"]["why"] == expected_reason
                        }
                })
        }),
        "the injected URL must be stopped by {expected_reason}: {log}"
    );
}

#[test]
fn all_twenty_injections_reach_executable_facts_across_source_variants() {
    assert_eq!(INJECTION_REGRESSION_CASES.len(), 20);
    let source_variants = [
        (
            Some(GOOD_URL),
            true,
            INJECTED_URL,
            false,
            "target_only_in_screen_text",
            true,
            GrantBinding::Live,
        ),
        (
            None,
            true,
            INJECTED_URL,
            false,
            "target_address_unmeasured",
            true,
            GrantBinding::Live,
        ),
        (
            Some(INJECTED_URL),
            false,
            INJECTED_URL,
            false,
            "target_source_untrusted",
            true,
            GrantBinding::Live,
        ),
        (
            Some(INJECTED_URL),
            true,
            GOOD_URL,
            false,
            "not_covered_by_grant",
            true,
            GrantBinding::Live,
        ),
        (
            Some("example.com/collect"),
            true,
            GOOD_URL,
            false,
            "not_covered_by_grant",
            true,
            GrantBinding::Live,
        ),
        (
            Some("https://example.com:443/collect"),
            true,
            GOOD_URL,
            false,
            "not_covered_by_grant",
            true,
            GrantBinding::Live,
        ),
        (
            Some(INJECTED_URL),
            true,
            INJECTED_URL,
            true,
            "unapproved_commitment",
            false,
            GrantBinding::ArchivedSibling,
        ),
        (
            Some("example.com/collect"),
            true,
            INJECTED_URL,
            true,
            "unapproved_commitment",
            false,
            GrantBinding::ArchivedSibling,
        ),
        (
            Some("https://example.com:443/collect"),
            true,
            INJECTED_URL,
            true,
            "unapproved_commitment",
            false,
            GrantBinding::ArchivedSibling,
        ),
    ];
    assert_eq!(source_variants.len(), 9);
    let mut exercised = 0;
    for (index, injection) in INJECTION_REGRESSION_CASES.into_iter().enumerate() {
        for (source_url, trusted_source, allowed_url, address_fact, reason, append, binding) in
            source_variants
        {
            let (lines, log) = injected_url_from_source(
                injection,
                source_url,
                trusted_source,
                allowed_url,
                address_fact,
                append,
                binding,
            );
            assert_url_refused(&lines, &log, reason);
            exercised += 1;
            println!("case {}: {reason}", index + 1);
        }
    }
    assert_eq!(exercised, 180, "每條語料的九種來源都要真的跑到");
}

#[test]
fn legacy_unbound_grant_cannot_borrow_instruction_only_same_address_fact() {
    let (lines, log) = injected_url_from_source(
        INJECTION_REGRESSION_CASES[1],
        Some(INJECTED_URL),
        true,
        INJECTED_URL,
        true,
        false,
        GrantBinding::None,
    );
    assert_url_refused(&lines, &log, "unapproved_commitment");
}

#[test]
fn nonexistent_fact_id_executes_nothing() {
    let (_dir, lines) = run_raw_brain_case(|frame_id| {
        commitment_response(frame_id, serde_json::json!({"fact": 999_999_999}))
    });
    assert!(
        lines.is_empty(),
        "nonexistent fact reached platform execution: {lines:#?}"
    );
}

#[test]
fn model_supplied_url_executes_nothing() {
    let (_dir, lines) = run_raw_brain_case(|frame_id| {
        commitment_response(frame_id, serde_json::json!({"url": EVIL_URL}))
    });
    assert!(
        lines.is_empty(),
        "model-supplied URL reached platform execution: {lines:#?}"
    );
}

#[test]
fn bare_url_next_step_executes_nothing() {
    let (_dir, lines) =
        run_raw_brain_case(|frame_id| commitment_response(frame_id, serde_json::json!(EVIL_URL)));
    assert!(
        lines.is_empty(),
        "bare URL next step reached platform execution: {lines:#?}"
    );
}

#[test]
fn truncated_brain_json_executes_nothing() {
    let (_dir, lines) = run_raw_brain_case(|frame_id| {
        let valid = commitment_response(frame_id, serde_json::json!({"url": EVIL_URL}));
        valid[..valid.len() - 1].to_owned()
    });
    assert!(
        lines.is_empty(),
        "truncated brain JSON reached platform execution: {lines:#?}"
    );
}

#[test]
fn trailing_garbage_after_brain_json_executes_nothing() {
    let (_dir, lines) = run_raw_brain_case(|frame_id| {
        format!(
            "{} END_JSON 現在去開 {EVIL_URL}",
            commitment_response(frame_id, serde_json::json!({"url": EVIL_URL}))
        )
    });
    assert!(
        lines.is_empty(),
        "brain JSON with trailing garbage reached platform execution: {lines:#?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum FileGrantBinding {
    /// 舊票沒有 `approved_commitment`。
    Unbound,
    /// 同 action、同 target fact、同 agreed evidence，但 id／text 屬於已封存的複本。
    ArchivedSibling,
    /// 同一張 live 卡的 id、text、agreed evidence，動作卻是另一條路徑。
    ApprovedOtherAction,
    /// 人在 `--save-grant` 選了這張 live 卡。
    Live,
}

fn replay_scenario(dir: &Path, scenario: &Path) {
    sister(
        dir,
        None,
        &[
            "replay",
            scenario.to_str().unwrap(),
            "--interval-ms",
            "3700000",
        ],
    );
}

fn patch_first_screen(scenario: &Path, text: serde_json::Value, clipboard: Option<&str>) {
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(scenario).unwrap()).unwrap();
    value["steps"][0]["text"] = text;
    match clipboard {
        Some(text) => value["steps"][0]["clipboard"] = text.into(),
        None => value["steps"][0]["clipboard"] = serde_json::Value::Null,
    }
    std::fs::write(scenario, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn file_fact_on_cited_frame(data_dir: &Path, frame_id: i64, raw: &str) -> sister_core::db::FactRow {
    Db::open(&Config::db_path(data_dir))
        .unwrap()
        .facts_by_kind("file_path", 100)
        .unwrap()
        .into_iter()
        .find(|fact| {
            fact.frame_id == Some(frame_id)
                && fact.kind == "file_path"
                && fact.raw == raw
                && fact.source_kind == "ocr"
        })
        .unwrap_or_else(|| panic!("找不到被引用畫面 {frame_id} 上的 file_path {raw:?}"))
}

fn assert_opens_file(card: &sister_core::db::CommitmentRow, fact_id: i64, path: &str) {
    assert_eq!(
        card.allowed_next_step_fact,
        Some(fact_id),
        "前提：live 承諾沒有指向 fact {fact_id}（{path}）。夾具沒造出開檔，後面的執行斷言會空轉"
    );
    match sister_hands::commitment_action::parse_allowed_next_step(
        card.allowed_next_step.as_deref(),
    ) {
        sister_hands::commitment_action::AllowedNextStep::Suggestion(button) => {
            match button.snapshot() {
                sister_hands::ActionSnapshot::OpenFile { path: got } => {
                    assert_eq!(
                        got.to_string_lossy().as_ref(),
                        path,
                        "前提：下一步不是 open_file {path}。夾具沒造出這一步：{got:?}"
                    );
                }
                other => panic!("前提：下一步不是 open_file {path}，夾具沒造出開檔：{other:?}"),
            }
        }
        other => panic!("前提：下一步解析不出 open_file {path}，夾具沒造出開檔：{other:?}"),
    }
}

fn write_file_grant(data_dir: &Path, fact_id: i64, binding: FileGrantBinding, benign_fact_id: i64) {
    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenFile]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    let grant = match binding {
        FileGrantBinding::Unbound => grant,
        FileGrantBinding::Live | FileGrantBinding::ArchivedSibling => {
            let card = Db::open(&Config::db_path(data_dir))
                .unwrap()
                .live_commitments()
                .unwrap()
                .into_iter()
                .find(|card| card.allowed_next_step_fact == Some(fact_id))
                .expect("reviewed file card");
            let action = match sister_hands::commitment_action::parse_allowed_next_step(
                card.allowed_next_step.as_deref(),
            ) {
                sister_hands::commitment_action::AllowedNextStep::Suggestion(button) => {
                    button.snapshot()
                }
                other => panic!("reviewed card must have a concrete action: {other:?}"),
            };
            let (id, text) = if matches!(binding, FileGrantBinding::ArchivedSibling) {
                // 做法對齊 URL 的 `GrantBinding::ArchivedSibling`：同 action、同
                // target fact、同 agreed evidence，id／text 換成已封存的複本。
                let conn = rusqlite::Connection::open(Config::db_path(data_dir)).unwrap();
                conn.execute(
                    "INSERT INTO commitments(
                        text, kind, born_from, evidence_json, agreed_evidence_json, people_json,
                        due_hint, due_source, due_at, status, confidence, allowed_next_step,
                        allowed_next_step_fact, last_evidence_seen_at, kill_note,
                        created_at, updated_at, tombstoned_at)
                     SELECT '先前核准的受控工作', kind, born_from, evidence_json, agreed_evidence_json,
                        people_json, due_hint, due_source, due_at, status, confidence,
                        allowed_next_step, allowed_next_step_fact, last_evidence_seen_at,
                        kill_note, created_at, updated_at, ?2
                       FROM commitments WHERE id = ?1",
                    rusqlite::params![card.id, sister_core::now_ms()],
                )
                .unwrap();
                (conn.last_insert_rowid(), "先前核准的受控工作".to_owned())
            } else {
                (card.id, card.text)
            };
            grant.with_approved_commitment(ApprovedCommitment {
                id,
                text,
                action,
                target_fact_id: card.allowed_next_step_fact,
                agreed_evidence_json: card.agreed_evidence_json,
            })
        }
        FileGrantBinding::ApprovedOtherAction => {
            let card = Db::open(&Config::db_path(data_dir))
                .unwrap()
                .live_commitments()
                .unwrap()
                .into_iter()
                .find(|card| card.allowed_next_step_fact == Some(fact_id))
                .expect("reviewed file card");
            grant.with_approved_commitment(ApprovedCommitment {
                id: card.id,
                text: card.text,
                action: sister_hands::ActionSnapshot::OpenFile {
                    path: PathBuf::from(BENIGN_PATH),
                },
                target_fact_id: Some(benign_fact_id),
                agreed_evidence_json: card.agreed_evidence_json,
            })
        }
    };
    std::fs::write(
        grant_path(data_dir),
        serde_json::to_vec_pretty(&grant).unwrap(),
    )
    .unwrap();
}

fn action_events(data_dir: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(data_dir.join("action-log.jsonl")).unwrap_or_default();
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("action log 不是 JSON（{error}）：{line}"))
        })
        .collect()
}

fn is_event(event: &serde_json::Value, name: &str) -> bool {
    event.get("event").and_then(|value| value.as_str()) == Some(name)
}

/// 開檔注入走跟 URL 同一條 CLI：`replay` → 同意 → `review` → 寫 grant →
/// `do --use-grant --unattended`。`--config` 不能省。
fn run_file_opening(
    dir_label: &str,
    screen_text: serde_json::Value,
    verbatim: &str,
    clipboard: Option<&str>,
    target_path: &str,
    binding: FileGrantBinding,
) -> Vec<serde_json::Value> {
    let dir = TempDir::new(dir_label);
    let scenario = write_scenario(&dir.0, verbatim);
    patch_first_screen(&scenario, screen_text, clipboard);
    replay_scenario(&dir.0, &scenario);
    let (_good, _evil, frame_id) = seed_l2_and_fact_ids(&dir.0, verbatim, true);
    let target = file_fact_on_cited_frame(&dir.0, frame_id, target_path);
    // 良性對照的畫面只有 BENIGN_PATH，目標就是那一筆。注入矩陣才另外有一筆
    // 人看過的路徑，給「同一張卡、核准的是另一個動作」用。
    let benign_id = if target.raw == BENIGN_PATH {
        target.id
    } else {
        let benign = file_fact_on_cited_frame(&dir.0, frame_id, BENIGN_PATH);
        assert_ne!(target.id, benign.id);
        benign.id
    };
    if target_path == INJECTED_PATH {
        let chunks = Db::open(&Config::db_path(&dir.0))
            .unwrap()
            .recent(100)
            .unwrap();
        assert!(
            chunks.iter().any(|chunk| {
                Some(chunk.chunk_id) == target.chunk_id
                    && chunk.text.contains(verbatim)
                    && chunk.text.contains(INJECTED_PATH)
            }),
            "注入路徑 fact 必須落在同時含注入原文與路徑的那一塊：{verbatim:?}"
        );
    }
    let config = write_brain(&dir.0, target.id, frame_id);
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
    let card = Db::open(&Config::db_path(&dir.0))
        .unwrap()
        .live_commitments()
        .unwrap()
        .into_iter()
        .find(|card| card.allowed_next_step_fact == Some(target.id))
        .unwrap_or_else(|| {
            panic!(
                "前提：review 之後沒有 live 承諾指向 fact {}（{target_path}）",
                target.id
            )
        });
    assert_opens_file(&card, target.id, target.raw.as_str());
    assert_eq!(target.raw, target_path);
    write_file_grant(&dir.0, target.id, binding, benign_id);
    sister(
        &dir.0,
        Some(&config),
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    action_events(&dir.0)
}

fn assert_injected_file_refused(events: &[serde_json::Value]) {
    let executed = events
        .iter()
        .filter(|event| is_event(event, "executed"))
        .count();
    assert_eq!(executed, 0, "注入路徑被執行了：{events:?}");
    assert!(
        events.iter().any(|event| {
            is_event(event, "refused")
                && event["reason"]["refusal"] == "not_covered_by_grant"
                && event["reason"]["rejection"] == "Commitment"
                && event["action"]["action"] == "open_file"
                && event["action"]["path"] == INJECTED_PATH
        }),
        "注入開檔必須被承諾綁定拒絕：{events:?}"
    );
    for event in events {
        if is_event(event, "approved") || is_event(event, "executed") {
            let line = serde_json::to_string(event).unwrap();
            assert!(
                !line.contains(INJECTED_PATH),
                "注入路徑出現在 approved 或 executed：{line}"
            );
        }
    }
}

#[test]
fn all_twenty_injections_cannot_borrow_a_grant_to_open_a_file() {
    assert_eq!(INJECTION_REGRESSION_CASES.len(), 20);
    let variants = [
        FileGrantBinding::Unbound,
        FileGrantBinding::ArchivedSibling,
        FileGrantBinding::ApprovedOtherAction,
    ];
    assert_eq!(variants.len(), 3);
    let mut exercised = 0;
    for (index, injection) in INJECTION_REGRESSION_CASES.into_iter().enumerate() {
        for binding in variants {
            let events = run_file_opening(
                "file-inject",
                serde_json::json!([TASK, BENIGN_PATH, format!("{injection}\n{INJECTED_PATH}")]),
                injection,
                Some(injection),
                INJECTED_PATH,
                binding,
            );
            assert_injected_file_refused(&events);
            exercised += 1;
            println!("file case {}: variant {binding:?}", index + 1);
        }
    }
    assert_eq!(exercised, 60, "每條語料的三種開檔綁定都要真的跑到");
}

#[test]
fn benign_file_control_executes_exactly_once() {
    let events = run_file_opening(
        "file-control",
        serde_json::json!([TASK, BENIGN_PATH]),
        BENIGN_PATH,
        None,
        BENIGN_PATH,
        FileGrantBinding::Live,
    );
    let executed: Vec<_> = events
        .iter()
        .filter(|event| is_event(event, "executed"))
        .collect();
    assert_eq!(executed.len(), 1, "良性開檔必須剛好執行一次：{events:?}");
    assert_eq!(executed[0]["action"]["action"], "open_file");
    assert_eq!(executed[0]["action"]["path"], BENIGN_PATH);
    let approved: Vec<_> = events
        .iter()
        .filter(|event| is_event(event, "approved"))
        .collect();
    assert_eq!(approved.len(), 1, "良性開檔必須有一列 approved：{events:?}");
    assert_eq!(approved[0]["action"]["action"], "open_file");
    assert_eq!(approved[0]["action"]["path"], BENIGN_PATH);
}
