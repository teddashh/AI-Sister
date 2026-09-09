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
    assert!(
        stdout.contains(&format!("--data-dir {} stop-all --off", dir.display())),
        "解除指令沒有帶這一趟真正使用的 data dir：{stdout}"
    );
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

#[test]
fn doctor_reports_master_pause_and_pull_without_hiding_any_state() {
    let master_only = temp("doctor-master-only");
    success(&master_only, None, &["stop-all"]);
    let doctor = success(&master_only, None, &["doctor"]);
    let hand = doctor
        .lines()
        .find(|line| line.contains("手 "))
        .expect("doctor hands row");
    assert!(hand.contains("手沒有拔掉"), "{hand}");
    assert!(hand.contains("三層全停"), "{hand}");
    let screen = doctor
        .lines()
        .find(|line| line.contains("讀你現在的螢幕"))
        .expect("doctor current-screen row");
    assert!(screen.contains("沒有抓畫面"), "{screen}");
    assert!(screen.contains("沒有對畫面做 OCR"), "{screen}");

    let paused = temp("doctor-master-pause");
    success(&paused, None, &["pause"]);
    success(&paused, None, &["stop-all"]);
    let doctor = success(&paused, None, &["doctor"]);
    let watching = doctor
        .lines()
        .find(|line| line.contains("現在有沒有在看"))
        .expect("doctor watching row");
    assert!(watching.contains("三層全停中"), "{watching}");
    assert!(watching.contains("暫停也還在"), "{watching}");
    assert!(
        watching.contains("解除全停後 capture 仍不會看"),
        "{watching}"
    );

    let both = temp("doctor-master-hands");
    success(&both, None, &["hands", "stop"]);
    success(&both, None, &["stop-all"]);
    let doctor = success(&both, None, &["doctor"]);
    let hand = doctor
        .lines()
        .find(|line| line.contains("手 "))
        .expect("doctor hands row");
    assert!(hand.contains("拔手與三層全停都在"), "{hand}");
    assert!(hand.contains("hands resume"), "{hand}");
    assert!(hand.contains("stop-all --off"), "{hand}");

    std::fs::remove_dir_all(master_only).unwrap();
    std::fs::remove_dir_all(paused).unwrap();
    std::fs::remove_dir_all(both).unwrap();
}

#[test]
fn pause_during_master_stop_says_resume_alone_cannot_restore_capture() {
    let dir = temp("pause-during-master");
    success(&dir, None, &["stop-all"]);
    let out = success(&dir, None, &["pause"]);
    assert!(out.contains("已暫停"), "{out}");
    assert!(out.contains("三層全停也還在"), "{out}");
    assert!(out.contains("只會解除暫停、救不了全停"), "{out}");
    assert!(out.contains("resume"), "{out}");
    assert!(out.contains("stop-all --off"), "{out}");
    assert!(!out.contains("要她繼續請跑"), "{out}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bench_refuses_before_capture_when_master_stop_is_on() {
    let dir = temp("bench");
    success(&dir, None, &["stop-all"]);
    let output = sister(&dir, None, &["bench", "--rounds", "1"]);
    assert!(!output.status.success(), "全停中 bench 不該成功");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(said.contains("bench 沒有抓畫面"), "{said}");
    assert!(
        said.contains(&format!("--data-dir {} stop-all --off", dir.display())),
        "{said}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// 打錯 `--data-dir` 的時候，全停不可以安靜地成功。
///
/// 對照組是同一顆二進位檔的 `hands stop`：它本來就拒絕。三層裡有一層會拒絕、
/// 而 `stop-all` 卻印「三層都停了」，那句話就是假的。
#[test]
fn stop_all_refuses_a_data_dir_that_is_not_there() {
    let base = temp("missing");
    let missing = base.join("not-here");

    let hands = sister(&missing, None, &["hands", "stop"]);
    assert!(!hands.status.success(), "對照組：hands stop 應該拒絕");

    let out = sister(&missing, None, &["stop-all"]);
    assert!(
        !out.status.success(),
        "全停對著不存在的資料夾成功了：\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let said = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(!said.contains("三層都停了"), "宣稱停了卻沒停：{said}");
    assert!(said.contains("找不到這個資料目錄"), "{said}");
    assert!(
        !said.contains("真正在跑"),
        "不存在的路徑不能證明另有一份正在跑：{said}"
    );
    assert!(!missing.exists(), "拒絕之後還是把資料夾建出來了");

    let off = sister(&missing, None, &["stop-all", "--off"]);
    assert!(!off.status.success(), "解除也不該對著不存在的資料夾成功");

    std::fs::remove_dir_all(base).unwrap();
}
