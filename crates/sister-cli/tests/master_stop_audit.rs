use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sister_core::model::{SystemEvent, SystemKind};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sister-master-stop-audit-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

fn sister(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sister"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run sister")
}

fn seeded(name: &str) -> PathBuf {
    let dir = temp(name);
    let mut db = sister_core::Db::open(&sister_core::Config::db_path(&dir)).expect("open db");
    let session = db.start_session("test", "0").expect("session");
    for (kind, ts) in [
        (SystemKind::MasterStopReleased, 1_000),
        (SystemKind::MasterStopEngaged, 2_000),
        (SystemKind::MasterStopReleased, 602_000),
        (SystemKind::MasterStopEngaged, 700_000),
    ] {
        db.insert_system(
            session,
            &SystemEvent {
                ts,
                kind,
                detail: None,
            },
        )
        .expect("insert master-stop event");
    }
    db.end_session(session).expect("end session");
    drop(db);
    dir
}

#[test]
fn stats_prints_master_stop_separately_without_rewording_pause() {
    let dir = seeded("human");
    let output = sister(&dir, &["stats"]);
    assert!(
        output.status.success(),
        "stats failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    assert!(
        stdout.contains("暫停      這份紀錄裡沒有暫停過"),
        "普通暫停的既有文案不可被全停改寫：{stdout}"
    );
    assert!(stdout.contains("全停      3 段"), "{stdout}");
    assert!(stdout.contains("已結束的加起來 10 分鐘"), "{stdout}");
    assert!(stdout.contains("最後一段沒有收尾"), "{stdout}");
    assert!(stdout.contains("其中 1 段的開頭已被保留期刪掉"), "{stdout}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn stats_json_has_a_distinct_master_stops_object() {
    let dir = seeded("json");
    let output = sister(&dir, &["stats", "--json"]);
    assert!(
        output.status.success(),
        "stats --json failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stats JSON");
    assert_eq!(value["pauses"]["episodes"], 0, "全停不可以灌進普通暫停");
    assert_eq!(
        value["master_stops"],
        serde_json::json!({
            "episodes": 3,
            "total_ms": 600_000,
            "open_since": 700_000,
            "truncated": 1,
        })
    );
    let _ = std::fs::remove_dir_all(dir);
}
