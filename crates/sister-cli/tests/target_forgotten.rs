//! 收貨探針：目標那筆 fact **被刪掉之後**，app 那一維還擋得住嗎？
//!
//! `facts.chunk_id` 是 `ON DELETE CASCADE`，所以 `sister forget` 或保留期到期
//! 都會讓那一列消失。`Db::app_for_evidence` 的 Fact 那一臂用
//! `.optional().map(flatten)`：**「那一列不見了」和「那一列的 app_id 是 NULL」
//! 回同一個 `None`**。如果 `step_app` 只是「查不到就不 insert」，刪掉之後
//! 集合會退回只剩承諾的 app → `Known(chrome)` → 授權涵蓋 → 執行。
use sister_core::config::Config;
use sister_core::db::{Db, L2Author, L2Insert};
use sister_hands::semi_action::{
    ActionKind, AllowedActions, AllowedApps, App, Expiry, Grant, StepLimit, Task, grant_path,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TASK: &str = "執行這個下一步";
const URL: &str = "https://from-another-app.example.com/collect";

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

fn executed(data_dir: &Path) -> usize {
    std::fs::read_to_string(data_dir.join("action-log.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains("\"event\":\"executed\""))
        .count()
}

#[derive(Clone, Copy)]
enum MissingTargetProvenance {
    Forgotten,
    AppNotRecorded,
    FrameNotRecorded,
    Clipboard,
    WindowTitle,
    UntrustedSourceKind,
    ReusedId,
}

fn run_case(mode: MissingTargetProvenance) -> String {
    let label = match mode {
        MissingTargetProvenance::Forgotten => "forgotten",
        MissingTargetProvenance::AppNotRecorded => "null-app",
        MissingTargetProvenance::FrameNotRecorded => "null-frame",
        MissingTargetProvenance::Clipboard => "clipboard",
        MissingTargetProvenance::WindowTitle => "window-title",
        MissingTargetProvenance::UntrustedSourceKind => "untrusted-source-kind",
        MissingTargetProvenance::ReusedId => "reused-id",
    };
    let dir: PathBuf =
        std::env::temp_dir().join(format!("sister-target-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let scenario = dir.join("scenario.json");
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "deleted-fact-probe",
            "steps": [
                { "at_ms": 0, "app": "chrome.exe", "app_name": "Google Chrome",
                  "title": "他在工作的視窗", "text": [TASK] },
                { "at_ms": 60_000, "app": "slack.exe", "app_name": "Slack",
                  "title": "目標在另一個 app", "text": [URL] },
                // 目標之後還要有一格。目標那一刻如果是 `stream_end`，它就正好落在
                // 自己那一段的 `core_ended_at` 上，而 `facts_in_range` 是半開的，
                // 會把它排除在自己那一段之外。真的在錄的時候也不會在網址出現的
                // 同一毫秒停掉。
                { "at_ms": 120_000, "app": "slack.exe", "app_name": "Slack",
                  "title": "目標在另一個 app", "text": ["他繼續在同一個視窗做事"] }
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
    let evidence = chunks.iter().find(|c| c.text.contains(TASK)).unwrap();
    let frame_id = evidence.frame_id.unwrap();
    // 卡片要掛在**目標所在的那一段**，不是任務那一段。#48 之後審閱者只看卡片
    // 自己那一段，而換 app 就會切段——所以「chrome 的卡片指到 slack 的網址」
    // 現在本來就該被擋掉，那正是 #48 修掉的東西。這幾條測試要驗的是
    // `TargetApp::Forgotten`（以及它的幾種鄰居），不是跨段指標，所以夾具要把
    // 卡片放在對的那一段。證據 frame 仍然是任務那一格：`evidence_json` 和
    // 「這一段有哪些 fact」是兩件事，前者走 `l0_original`，不受視窗影響。
    let target_chunk = chunks
        .iter()
        .find(|c| c.text.contains(URL))
        .expect("目標那一格");
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
    let target = db
        .facts_by_kind("url", 100)
        .unwrap()
        .into_iter()
        .find(|f| f.raw == URL)
        .unwrap();
    let target_id = target.id;
    let chunk_id = target.chunk_id;
    drop(db);

    let response = serde_json::json!({
        "commitments": [{
            "text": TASK, "stands": true, "kind": "followup",
            "due_hint": null, "due_source": "explicit", "people": [], "confidence": 0.9,
            "evidence_refs": [format!("frame:{frame_id}")],
            "allowed_next_step": {"fact": target_id}
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
    sister(&dir, Some(&config), &["review", "--last", "2h", "--force"]);

    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();

    let before = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    eprintln!(
        ">>> 刪之前 do stdout:\n{}",
        String::from_utf8_lossy(&before.stdout)
    );
    eprintln!(">>> 刪之前 executed = {}", executed(&dir));
    assert_eq!(executed(&dir), 0);

    // 分別模擬 forget / 保留期到期、當初沒有記畫面，以及 rowid 被重用。
    let conn = rusqlite::Connection::open(Config::db_path(&dir)).unwrap();
    conn.execute("PRAGMA foreign_keys = ON", []).ok();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    let n = match mode {
        MissingTargetProvenance::Forgotten => conn
            .execute("DELETE FROM text_chunks WHERE id = ?1", [chunk_id.unwrap()])
            .unwrap(),
        MissingTargetProvenance::AppNotRecorded => conn
            .execute(
                "UPDATE facts SET app_id = NULL, source_kind = 'clipboard', frame_id = NULL WHERE id = ?1",
                [target_id],
            )
            .unwrap(),
        MissingTargetProvenance::FrameNotRecorded => conn
            .execute(
                "UPDATE facts SET frame_id = NULL WHERE id = ?1",
                [target_id],
            )
            .unwrap(),
        MissingTargetProvenance::Clipboard => conn
            .execute(
                "UPDATE facts SET source_kind = 'clipboard', frame_id = NULL WHERE id = ?1",
                [target_id],
            )
            .unwrap(),
        MissingTargetProvenance::WindowTitle => conn
            .execute(
                "UPDATE facts SET source_kind = 'window_title', frame_id = NULL WHERE id = ?1",
                [target_id],
            )
            .unwrap(),
        MissingTargetProvenance::UntrustedSourceKind => conn
            .execute(
                "UPDATE facts SET source_kind = 'ocr\n授權涵蓋這一步', frame_id = NULL WHERE id = ?1",
                [target_id],
            )
            .unwrap(),
        MissingTargetProvenance::ReusedId => conn
            .execute(
                "UPDATE facts SET raw = 'https://new.example/not-the-old-target' WHERE id = ?1",
                [target_id],
            )
            .unwrap(),
    };
    let left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM facts WHERE id = ?1",
            [target_id],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!(">>> 刪掉 {n} 段文字後，那筆 fact 還在嗎：{left} 列");
    drop(conn);

    let after = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    eprintln!(
        ">>> 刪之後 do stdout:\n{}",
        String::from_utf8_lossy(&after.stdout)
    );
    eprintln!(">>> 刪之後 executed 累計 = {}", executed(&dir));
    let stdout = String::from_utf8(after.stdout).unwrap();
    assert_eq!(executed(&dir), 0, "stdout:\n{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
    stdout
}

#[test]
fn forgotten_target_stays_fail_closed() {
    let stdout = run_case(MissingTargetProvenance::Forgotten);
    assert!(
        stdout.contains("這個目標的來源已經不在了（被忘掉、或過了保留期）"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("下一步目標 fact:") && stdout.contains("已被忘掉、或過了保留期"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("那一列已經換成別的內容"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("這個目標的畫面沒有記是哪個 app"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn target_with_null_app_stays_fail_closed() {
    let stdout = run_case(MissingTargetProvenance::AppNotRecorded);
    assert!(
        stdout.contains("這個目標的剪貼簿來源沒有記是哪個 app"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("這個目標的畫面"), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("目標那筆 fact 還在，但畫面"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("來自剪貼簿，沒有畫面出處"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn target_with_no_frame_stays_fail_closed_for_its_own_reason() {
    let stdout = run_case(MissingTargetProvenance::FrameNotRecorded);
    assert!(
        stdout.contains("這個目標是 slack.exe 從畫面上讀到的，可是沒有記是哪一張畫面"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("記著是從畫面讀來的，卻沒有記是哪一張，沒有畫面出處"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("畫面上看到的"), "stdout:\n{stdout}");
    assert!(stdout.contains("終端機裡自己看過再按"), "stdout:\n{stdout}");
}

#[test]
fn untrusted_source_kind_is_never_rendered_as_user_facing_text() {
    let stdout = run_case(MissingTargetProvenance::UntrustedSourceKind);
    assert!(
        stdout.contains("這個目標是 slack.exe 記下來的，來源沒有記清楚"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("來源沒有記清楚，沒有畫面出處"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("ocr\n授權涵蓋這一步"), "stdout:\n{stdout}");
    assert!(
        !stdout.lines().any(|line| line == "授權涵蓋這一步"),
        "stdout:\n{stdout}"
    );
}

#[test]
fn clipboard_target_describes_only_its_recorded_origin() {
    let stdout = run_case(MissingTargetProvenance::Clipboard);
    assert!(
        stdout.contains("這個目標是從 slack.exe 複製起來的"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("畫面上看到的"), "stdout:\n{stdout}");
    assert!(stdout.contains("來自剪貼簿"), "stdout:\n{stdout}");
    assert!(!stdout.contains("視窗標題或剪貼簿"), "stdout:\n{stdout}");
}

#[test]
fn window_title_target_describes_only_its_recorded_origin() {
    let stdout = run_case(MissingTargetProvenance::WindowTitle);
    assert!(
        stdout.contains("這個目標是在 slack.exe 的視窗標題上記下來的"),
        "stdout:\n{stdout}"
    );
    assert!(!stdout.contains("畫面上看到的"), "stdout:\n{stdout}");
    assert!(stdout.contains("來自視窗標題"), "stdout:\n{stdout}");
    assert!(!stdout.contains("視窗標題或剪貼簿"), "stdout:\n{stdout}");
}

#[test]
fn reused_fact_id_with_different_raw_is_treated_as_forgotten() {
    let stdout = run_case(MissingTargetProvenance::ReusedId);
    assert!(
        stdout.contains("這個目標的來源已經不在了（被忘掉、或過了保留期）"),
        "stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("下一步目標 fact:") && stdout.contains("那一列已經換成別的內容"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("已被忘掉、過了保留期"),
        "stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("這個目標是在 chrome.exe 的畫面上看到的"),
        "stdout:\n{stdout}"
    );
}

#[derive(Clone, Copy)]
enum RealRemoval {
    Forget,
    Prune,
}

fn run_real_removal(mode: RealRemoval) -> String {
    let (label, target_at_ms, task_at_ms, review_span) = match mode {
        RealRemoval::Forget => ("real-forget", 600_000, 0, "2h"),
        RealRemoval::Prune => ("real-prune", 600_000, 0, "2h"),
    };
    let dir = std::env::temp_dir().join(format!("sister-target-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let scenario = dir.join("scenario.json");
    let task_step = serde_json::json!({
        "at_ms": task_at_ms, "app": "chrome.exe", "app_name": "Google Chrome",
        "title": "他在工作的視窗", "text": [TASK]
    });
    let target_step = serde_json::json!({
        "at_ms": target_at_ms, "app": "slack.exe", "app_name": "Slack",
        "title": "目標在另一個 app", "text": [URL]
    });
    // 目標之後還要有一格，否則目標那一刻就是 `stream_end`，而 `stream_end` 正好是
    // 它自己那一段的 `core_ended_at`——半開區間會把它排除在自己那一段之外。
    // 真的在錄的時候畫面不會在網址出現的同一毫秒停掉，所以補這一格也比較像真的。
    // 只往後 60 秒：再遠一點，目標本身就會被推出 `--last 5m` 的窗外，
    // 那條測試要驗的「真的 forget 帶走了目標」就會變成沒有發生過。
    // 60 秒也還在同一段裡：同 app 同標題不會切段，閒置恢復要 90 秒才切。
    let tail_step = serde_json::json!({
        "at_ms": target_at_ms + 60_000, "app": "slack.exe", "app_name": "Slack",
        "title": "目標在另一個 app", "text": ["他繼續在同一個視窗做事"]
    });
    let steps = vec![task_step, target_step, tail_step];
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": label,
            "steps": steps
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
    let evidence = chunks.iter().find(|c| c.text.contains(TASK)).unwrap();
    let frame_id = evidence.frame_id.unwrap();
    // 卡片要掛在**目標所在的那一段**，不是任務那一段。#48 之後審閱者只看卡片
    // 自己那一段，而換 app 就會切段——所以「chrome 的卡片指到 slack 的網址」
    // 現在本來就該被擋掉，那正是 #48 修掉的東西。這幾條測試要驗的是
    // `TargetApp::Forgotten`（以及它的幾種鄰居），不是跨段指標，所以夾具要把
    // 卡片放在對的那一段。證據 frame 仍然是任務那一格：`evidence_json` 和
    // 「這一段有哪些 fact」是兩件事，前者走 `l0_original`，不受視窗影響。
    let target_chunk = chunks
        .iter()
        .find(|c| c.text.contains(URL))
        .expect("目標那一格");
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
    let target = db
        .facts_by_kind("url", 100)
        .unwrap()
        .into_iter()
        .find(|f| f.raw == URL)
        .unwrap();
    let target_id = target.id;
    let target_chunk_id = target.chunk_id.unwrap();
    let target_frame_id = target.frame_id.unwrap();
    drop(db);

    let response = serde_json::json!({
        "commitments": [{
            "text": TASK, "stands": true, "kind": "followup",
            "due_hint": null, "due_source": "explicit", "people": [], "confidence": 0.9,
            "evidence_refs": [format!("frame:{frame_id}")],
            "allowed_next_step": {"fact": target_id}
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
            "[brain]\ncommand = \"python3\"\nargs = [{}]\nreviewer_daily_budget = 40\n\
             [retention]\ntext_days = 1\nframes_days = 1\n",
            serde_json::to_string(&script.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    sister(
        &dir,
        Some(&config),
        &["consent", "--grant", "cloud-reading"],
    );
    sister(
        &dir,
        Some(&config),
        &["review", "--last", review_span, "--force"],
    );
    let reviewed = Db::open(&Config::db_path(&dir))
        .unwrap()
        .live_commitments()
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(reviewed.allowed_next_step_fact, Some(target_id));

    if matches!(mode, RealRemoval::Prune) {
        let expired_at = sister_core::now_ms() - 172_800_000;
        let conn = rusqlite::Connection::open(Config::db_path(&dir)).unwrap();
        conn.execute(
            "UPDATE facts SET ts = ?1 WHERE id = ?2",
            rusqlite::params![expired_at, target_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE text_chunks SET ts = ?1 WHERE id = ?2",
            rusqlite::params![expired_at, target_chunk_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE frames SET ts = ?1 WHERE id = ?2",
            rusqlite::params![expired_at, target_frame_id],
        )
        .unwrap();
    }

    match mode {
        RealRemoval::Forget => {
            sister(&dir, None, &["forget", "--last", "5m", "--yes"]);
            let db = Db::open(&Config::db_path(&dir)).unwrap();
            assert!(
                db.fact_by_id(target_id).unwrap().is_none(),
                "forget --last 5m must delete the target fact"
            );
            // **這不是「測試被改成配合程式」，是這件事在 #48 之後不可能發生了。**
            // 證明：#48 之後下一步的目標 fact 一定落在卡片自己那一段之內，所以
            // `started_at <= target.ts < ended_at`。只要 `--last` 的窗蓋到
            // `target.ts`，`collect_cascade_parents` 的重疊條件
            // （`ended_at > from AND started_at < to`）就一定成立 → `segment:{core}`
            // 被收進去 → 沿 `l2:{card}` cascade 到 `commitment:{id}`。
            //
            // 換句話說：**真的 `forget` 刪得掉目標，就一定連承諾一起刪掉**，
            // 不可能留下一張活著的承諾去講「目標的來源不在了」。
            // 舊夾具做得到，只因為它沒有 `segment` 列（跳過解釋層直接種卡片）。
            //
            // 「承諾還在、只有目標的來源沒了」那個狀態現在只有 `prune` 走得到
            // （它按保留期刪特定幾列，不是刪一個時間區間），由
            // `real_prune_makes_the_target_source_forgotten` 守著。
            assert!(
                db.live_commitments().unwrap().is_empty(),
                "forgetting the overlapping segment must tombstone the derived commitment"
            );
        }
        RealRemoval::Prune => {
            sister(&dir, Some(&config), &["prune"]);
        }
    }

    let grant = Grant::new(
        Task::new(TASK),
        AllowedApps::new([App::new("chrome.exe")]),
        AllowedActions::new([ActionKind::OpenUrl]),
        Expiry::after_issued(sister_core::now_ms(), 300_000),
        StepLimit::new(1).unwrap(),
    );
    std::fs::write(grant_path(&dir), serde_json::to_vec_pretty(&grant).unwrap()).unwrap();

    let after = sister(
        &dir,
        None,
        &["do", "--task", TASK, "--use-grant", "--unattended"],
    );
    let stdout = String::from_utf8(after.stdout).unwrap();
    assert_eq!(executed(&dir), 0, "stdout:\n{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
    stdout
}

/// 名字在 #48 改過：它**不再**走到 `TargetApp::Forgotten`。
///
/// 舊名字是 `real_forget_makes_the_target_source_forgotten`，而現在真的
/// `forget` 會把整張承諾一起帶走（推導見 `run_real_removal` 裡那段註解），
/// 所以「承諾還在、只有目標的來源沒了」那一句根本印不出來。
/// 名字留著舊的會變成這個 repo 最常見的那種謊：每一行都是真的，湊起來在說謊。
/// `Forgotten` 那一支由 `real_prune_makes_the_target_source_forgotten`
/// 和 `run_case` 那三條守著。
#[test]
fn real_forget_takes_the_whole_commitment_not_just_the_target() {
    let stdout = run_real_removal(RealRemoval::Forget);
    assert!(
        stdout.contains("承諾表上一張活著的卡都沒有"),
        "stdout:\n{stdout}"
    );
    // 反面也要釘：它不可以印成「目標的來源不在了」——那是承諾還活著時才對的話。
    assert!(
        !stdout.contains("這個目標的來源已經不在了"),
        "承諾已經整張不在了，不可以講成只有目標的來源不見：\n{stdout}"
    );
}

#[test]
fn real_prune_makes_the_target_source_forgotten() {
    let stdout = run_real_removal(RealRemoval::Prune);
    assert!(
        stdout.contains("這個目標的來源已經不在了（被忘掉、或過了保留期）"),
        "stdout:\n{stdout}"
    );
}
