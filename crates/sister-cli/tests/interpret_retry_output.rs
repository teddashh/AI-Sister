use sister_core::config::Config;
use sister_core::db::{Db, OutboundInsert};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn sister(data_dir: &Path, config: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sister"));
    command.arg("--data-dir").arg(data_dir);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.args(args);
    command.output().expect("run sister")
}

fn success(data_dir: &Path, config: Option<&Path>, args: &[&str]) -> String {
    let output = sister(data_dir, config, args);
    assert!(
        output.status.success(),
        "sister {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 stdout")
}

#[test]
fn second_real_interpret_prints_retry_line_and_whole_run_warning() {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "sister-interpret-retry-output-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp data dir");

    let scenario = dir.join("scenario.json");
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "interpret-retry-output",
            "steps": [
                {"at_ms": 0, "app": "code.exe", "title": "compiler", "text": ["error[E0308]: mismatched types"]},
                {"at_ms": 180000, "app": "chrome.exe", "title": "docs", "text": ["查型別文件"]},
                {"at_ms": 200000, "app": "chrome.exe", "title": "docs continued", "text": ["繼續閱讀"]}
            ]
        }))
        .expect("scenario json"),
    )
    .expect("write scenario");

    success(&dir, None, &["consent", "--grant", "local-recording"]);
    success(&dir, None, &["consent", "--grant", "cloud-reading"]);
    success(
        &dir,
        None,
        &[
            "replay",
            scenario.to_str().expect("scenario path"),
            "--interval-ms",
            "60000",
        ],
    );

    let fake = dir.join("fake-brain.py");
    std::fs::write(&fake, "import sys\nsys.stdin.buffer.read()\n").expect("write fake brain");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[brain]\ncommand = \"python3\"\nargs = [{}]\n",
            serde_json::to_string(&fake.to_string_lossy()).expect("quote fake path")
        ),
    )
    .expect("write config");

    let first = success(&dir, Some(&config), &["interpret", "--last", "24h"]);

    // 第二輪之前讓兩段的舊次數不同，這樣 stdout 必須把次數
    // 掛在各自的 segment 底下，不能永遠讀 ran[0]。
    let mut db = Db::open(&Config::db_path(&dir)).expect("open replay db");
    let chunks = db.recent(100).expect("recent chunks");
    let chunk = chunks
        .iter()
        .find(|c| c.text.contains("E0308"))
        .expect("error chunk");
    let frame_id = chunk.frame_id.expect("error frame");
    let segs = db
        .chapters_for_range(chunk.ts, chunk.ts + 240_000)
        .expect("chapters");
    assert!(segs.len() >= 2, "歸屬測試需要兩段：{segs:?}");
    let core = segs[0].core_started_at;
    let other_core = segs[1].core_started_at;
    db.conn()
        .execute(
            "INSERT INTO brain_outbound(ts, day_key, command, args_json,
         segment_core_start, chars_sent, truncated, outcome, duration_ms, role)
         VALUES(?1, '2026-01-01', 'seed', '[]', ?2, 1, 0, 'timeout', 1, 'interpreter')",
            rusqlite::params![chunk.ts - 1, core],
        )
        .expect("add one extra prior attempt");
    drop(db);
    let second = success(&dir, Some(&config), &["interpret", "--last", "24h"]);

    for absent in ["第 2 次問這一段", "沒寫成卡片", "還會再問", "外送額度"] {
        assert!(!first.contains(absent), "第一次不該有 {absent}：\n{first}");
    }
    for required in ["第 2 次問這一段", "沒寫成卡片", "還會再問", "外送額度"] {
        assert!(
            second.contains(required),
            "第二次少了 {required}：\n{second}"
        );
    }
    let block = |stdout: &str, segment: i64| {
        let marker = format!("── segment:{segment} ──");
        let start = stdout
            .find(&marker)
            .unwrap_or_else(|| panic!("找不到 {marker}:\n{stdout}"));
        let rest = &stdout[start..];
        let end = rest[marker.len()..]
            .find("── segment:")
            .map_or(rest.len(), |i| marker.len() + i);
        rest[..end].to_owned()
    };
    assert!(
        block(&second, core).contains("第 3 次問這一段"),
        "第一段數字掛錯：\n{second}"
    );
    assert!(
        block(&second, other_core).contains("第 2 次問這一段"),
        "第二段數字掛錯：\n{second}"
    );

    let at_failed = success(
        &dir,
        Some(&config),
        &["interpret", "--at", &core.to_string()],
    );
    for required in [
        "--at",
        "所以不一定會有下一次。",
        "外送時間",
        "每醒來一次",
        "重開",
    ] {
        assert!(
            at_failed.contains(required),
            "--at 實際 stdout 少了 {required}:\n{at_failed}"
        );
    }
    assert!(
        !at_failed.contains("下一輪還會再問"),
        "--at 不可承諾下一輪還會再問：\n{at_failed}"
    );
    assert!(!at_failed.contains("掉出時間範圍"), "{at_failed}");

    // 把既有列改成未來版本的 token，再讓真的子指令收回合法卡片。這同時守住：
    // 未知 token 不可炸掉 interpret，也不可擋住新的稽核列與 L2 卡片。
    let db = Db::open(&Config::db_path(&dir)).expect("open replay db");
    db.conn()
        .execute(
            "UPDATE brain_outbound SET outcome = 'future_token'
             WHERE segment_core_start = ?1 AND role = 'interpreter'",
            [core],
        )
        .expect("plant unknown token");
    let before = db.list_brain_outbound(100).expect("outbound before").len();
    drop(db);
    let card = format!(
        r#"{{"segment_ref":"segment:{core}","activity":"修好型別錯誤","entities":[],"confidence":0.7,"evidence_refs":["frame:{frame_id}"],"open_questions":[]}}"#
    );
    std::fs::write(
        &fake,
        format!(
            "import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write({card:?}.encode('utf-8'))\n"
        ),
    )
    .expect("rewrite successful fake brain");
    let third = success(
        &dir,
        Some(&config),
        &["interpret", "--at", &core.to_string()],
    );
    assert!(third.contains("成功，寫進 L2"), "{third}");
    assert!(third.contains("這一次成功了"), "{third}");
    let db = Db::open(&Config::db_path(&dir)).expect("reopen after unknown token");
    assert_eq!(
        db.list_brain_outbound(100).expect("outbound after").len(),
        before + 1,
        "未知 token 不可擋住新的外送稽核列"
    );
    assert!(
        db.latest_l2_for_segment(core).expect("latest L2").is_some(),
        "未知 token 不可擋住新卡片"
    );

    eprintln!("AT_FAILED_STDOUT_BEGIN\n{at_failed}AT_FAILED_STDOUT_END");
    eprintln!("LAST_FAILED_STDOUT_BEGIN\n{second}LAST_FAILED_STDOUT_END");
    eprintln!("RETRIED_SUCCESS_STDOUT_BEGIN\n{third}RETRIED_SUCCESS_STDOUT_END");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_brain_log_after_a_send_names_only_the_actual_deletion_path() {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "sister-brain-log-empty-after-send-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp data dir");

    let mut db = Db::open(&Config::db_path(&dir)).expect("open db");
    db.insert_brain_outbound(&OutboundInsert {
        ts: 1_700_000_000_000,
        day_key: "2023-11-14",
        command: "fake-agent",
        args: &[],
        segment_core_start: Some(1_700_000_000_000),
        chars_sent: 1,
        truncated: false,
        outcome: "timeout",
        duration_ms: 1,
        error: None,
        role: "interpreter",
    })
    .expect("seed outbound history");
    db.conn()
        .execute("DELETE FROM brain_outbound", [])
        .expect("simulate forget removing retained outbound rows");
    drop(db);

    let stdout = success(&dir, None, &["brain", "log"]);
    assert!(
        stdout.contains(
            "送過，但那些列已經被 `sister forget` 依外送時間（問出去那一刻）清掉了。不是從來沒送。"
        ),
        "brain log 沒有講清楚實際刪除路徑：\n{stdout}"
    );
    for false_cause in ["保留期", "prune"] {
        assert!(
            !stdout.contains(false_cause),
            "brain log 不可宣稱 {false_cause} 會清 brain_outbound：\n{stdout}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
