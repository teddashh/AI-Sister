use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn sister(data_dir: &Path, config: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sister"));
    command.arg("--data-dir").arg(data_dir);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.args(args).output().expect("run sister")
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

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sister-master-stop-cli-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seeded(name: &str) -> PathBuf {
    let dir = temp(name);
    let scenario = dir.join("scenario.json");
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "master-stop-blocks-brain",
            "privacy_context": "clear",
            "system_state": "active",
            "steps": [
                {"at_ms": 0, "app": "code.exe", "title": "compiler", "text": ["error[E0308]: mismatched types"]},
                {"at_ms": 180000, "app": "chrome.exe", "title": "docs", "text": ["查型別文件"]},
                {"at_ms": 200000, "app": "chrome.exe", "title": "docs continued", "text": ["繼續閱讀"]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    success(&dir, None, &["consent", "--grant", "local-recording"]);
    success(&dir, None, &["consent", "--grant", "cloud-reading"]);
    success(
        &dir,
        None,
        &[
            "replay",
            scenario.to_str().unwrap(),
            "--interval-ms",
            "60000",
        ],
    );
    dir
}

fn fake_brain(dir: &Path, card_for: Option<(i64, i64)>) -> PathBuf {
    let fake = dir.join("fake-brain.py");
    let body = match card_for {
        None => "import sys\nsys.stdin.buffer.read()\n".to_owned(),
        Some((core, frame)) => {
            let card = format!(
                r#"{{"segment_ref":"segment:{core}","activity":"修好型別錯誤","entities":[],"confidence":0.7,"evidence_refs":["frame:{frame}"],"open_questions":[]}}"#
            );
            format!(
                "import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write({card:?}.encode('utf-8'))\n"
            )
        }
    };
    std::fs::write(&fake, body).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[brain]\ncommand = \"python3\"\nargs = [{}]\n",
            serde_json::to_string(&fake.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    config
}

fn counts(dir: &Path) -> (usize, usize) {
    let db = sister_core::Db::open(&sister_core::Config::db_path(dir)).unwrap();
    let sent = db
        .list_brain_outbound(1000)
        .unwrap()
        .iter()
        .map(|row| row.chars_sent.max(0) as usize)
        .sum();
    let cards: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM l2_card", [], |row| row.get(0))
        .unwrap();
    (sent, cards as usize)
}

#[test]
fn master_stop_blocks_brain_bytes_and_cards_and_says_why() {
    let dir = seeded("brain");
    let config = fake_brain(&dir, None);
    let _ = success(&dir, Some(&config), &["interpret", "--last", "24h"]);
    let (core, frame) = {
        let db = sister_core::Db::open(&sister_core::Config::db_path(&dir)).unwrap();
        let core = db
            .conn()
            .query_row(
                "SELECT core_started_at FROM segment ORDER BY core_started_at LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let frame = db
            .conn()
            .query_row(
                "SELECT frame_id FROM facts WHERE frame_id IS NOT NULL LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        (core, frame)
    };
    let config = fake_brain(&dir, Some((core, frame)));
    let before = counts(&dir);
    let stopped = success(&dir, None, &["stop-all"]);
    assert!(stopped.contains("三層都停了"), "{stopped}");
    assert!(stopped.contains("不會自己恢復"), "{stopped}");
    let stdout = success(&dir, Some(&config), &["interpret", "--last", "24h"]);
    assert_eq!(counts(&dir), before, "全停後仍送字或寫卡：\n{stdout}");
    assert!(
        stdout.contains("三層全停中"),
        "沒有把沒問模型的理由講出來：{stdout}"
    );
    assert!(stdout.contains("沒有問模型"), "{stdout}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn releasing_master_stop_preserves_an_earlier_pause_and_hands_pull() {
    let paused = temp("preserve-pause");
    success(&paused, None, &["pause"]);
    success(&paused, None, &["stop-all"]);
    let out = success(&paused, None, &["stop-all", "--off"]);
    assert!(sister_core::pause::is_paused(&paused));
    assert!(out.contains("原本自己按的暫停還在"), "{out}");

    let hands = temp("preserve-hands");
    success(&hands, None, &["hands", "stop"]);
    success(&hands, None, &["stop-all"]);
    let out = success(&hands, None, &["stop-all", "--off"]);
    assert!(sister_hands::kill_switch::is_pulled(&hands));
    assert!(out.contains("原本自己按的拔手還在"), "{out}");

    std::fs::remove_dir_all(paused).unwrap();
    std::fs::remove_dir_all(hands).unwrap();
}

#[test]
fn resume_during_master_stop_names_both_true_states_and_the_lever() {
    let dir = temp("resume");
    success(&dir, None, &["pause"]);
    success(&dir, None, &["stop-all"]);
    let out = success(&dir, None, &["resume"]);
    assert!(out.contains("暫停已解除"), "{out}");
    assert!(out.contains("三層全停還在"), "{out}");
    assert!(out.contains("capture 仍然停著"), "{out}");
    assert!(out.contains("stop-all --off"), "{out}");
    assert!(!sister_core::pause::is_paused(&dir));
    assert!(sister_hands::master_stop::is_stopped(&dir));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn hands_resume_during_master_stop_does_not_claim_hands_can_act() {
    let dir = temp("hands-resume");
    success(&dir, None, &["hands", "stop"]);
    success(&dir, None, &["stop-all"]);
    let out = success(&dir, None, &["hands", "resume"]);
    assert!(out.contains("已把手接回去"), "{out}");
    assert!(out.contains("三層全停還在"), "{out}");
    assert!(out.contains("hands 仍然停著"), "{out}");
    assert!(out.contains("stop-all --off"), "{out}");
    assert!(!sister_hands::kill_switch::is_pulled(&dir));
    assert!(sister_hands::master_stop::is_stopped(&dir));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stop_all_twice_preserves_timestamp_and_doctor_never_calls_it_running() {
    let dir = seeded("doctor");
    success(&dir, None, &["stop-all"]);
    let first = sister_hands::master_stop::stopped_since(&dir).unwrap();
    let again = success(&dir, None, &["stop-all"]);
    assert_eq!(sister_hands::master_stop::stopped_since(&dir), Some(first));
    assert!(again.contains("本來就全停著"), "{again}");
    let doctor = success(&dir, None, &["doctor"]);
    assert!(doctor.contains("capture／brain／hands 全停"), "{doctor}");
    assert!(doctor.contains("三層全停中"), "{doctor}");
    let watching = doctor
        .lines()
        .find(|line| line.contains("現在有沒有在看"))
        .expect("doctor watching row");
    assert!(!watching.contains("正在跑"), "全停中卻說正在跑：{watching}");
    std::fs::remove_dir_all(dir).unwrap();
}
