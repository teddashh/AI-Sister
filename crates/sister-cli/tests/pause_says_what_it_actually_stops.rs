//! 暫停那句話，要對著暫停**實際上**停掉的東西講。
//!
//! 這一檔存在的理由是一次實測。`sister pause` 之後跑 `sister interpret`：
//!
//! ```text
//! ===== 暫停前的快照 =====   brain_outbound 2   l2_card 0
//! ===== sister pause =====
//! ===== 暫停中跑 interpret =====
//!   結局：成功，寫進 L2（20 ms）
//! ===== 暫停後 =====         brain_outbound 4   l2_card 1
//!   外送: [(…, 1008, 'success'), (…, 968, 'bad_json')]
//! ```
//!
//! 暫停中送出去 1976 個字、寫進 1 張新卡片、多了 2 列外送稽核。而 `sister
//! doctor` 那一列當時寫著「這段期間**什麼都不會被記錄**」。
//!
//! 成因不是哪裡壞了，是**沒有人問過**：`ops::interpret`、`ops::brain`、
//! `ops::review` 三個模組加起來 0 處 `is_paused`；`ops::watch` 那 2000 行只有
//! 一處（`blind_reason`，那是一支解釋「為什麼看不到新畫面」的顯示函式，不擋
//! 任何寫入）。擷取那半是真的停了——`recorder.rs:538` 一進 `tick_inner` 就
//! `return Ok(Tick::Paused)`，連沒有內容的輸入節奏都不累積。
//!
//! 所以暫停停的是**眼睛**，不是**嘴巴**。桌面那半一直是對的（`app.js:16`
//! 「已暫停，沒有在看」、`:408`「想一下…（仍在暫停）」），CLI 這半在說謊。
//!
//! **這一檔的兩條測試要一起讀**：一條量行為、一條讀句子，跑在同一個資料目錄
//! 上。分開寫的話，句子改對了而行為變了（或反過來）不會有人紅。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 這一版承諾的那句話，逐字。分成兩半是因為它們證明的是不同的事：
/// 前半是「她停了什麼」，後半是「她沒停什麼」。少任何一半都是舊 bug。
const STOPPED: &str = "她不會再看新的畫面";
const NOT_STOPPED: &str = "已經記下來的";
/// 沒有這一段的話，那句話只是一個沒有下一步的壞消息。
const THE_LEVER: &str = "consent --revoke cloud-reading";
/// 上一版那句假話。它不准再出現在任何 CLI 輸出裡。
const THE_OLD_LIE: &str = "什麼都不會被記錄";

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

fn counts(dir: &Path) -> (usize, usize) {
    use sister_core::config::Config;
    use sister_core::db::Db;
    let db = Db::open(&Config::db_path(dir)).expect("open db");
    let outbound = db.list_brain_outbound(1000).expect("outbound").len();
    let cards: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM l2_card", [], |r| r.get(0))
        .expect("count cards");
    (outbound, cards as usize)
}

/// 種一個「有東西可以解讀」的資料目錄，回傳它。
fn seeded(name: &str) -> PathBuf {
    let dir: PathBuf = std::env::temp_dir().join(format!("sister-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp data dir");

    let scenario = dir.join("scenario.json");
    std::fs::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "pause-does-not-stop-the-brain",
            "steps": [
                {"at_ms": 0, "app": "code.exe", "title": "compiler",
                 "text": ["error[E0308]: mismatched types"]},
                {"at_ms": 180000, "app": "chrome.exe", "title": "docs", "text": ["查型別文件"]},
                {"at_ms": 200000, "app": "chrome.exe", "title": "docs continued",
                 "text": ["繼續閱讀"]}
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
    dir
}

/// 大腦 CLI：先跑一支什麼都不回的（把 segment 算出來），再換成會回一張合法
/// 卡片的。回傳 config 路徑。
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
    std::fs::write(&fake, body).expect("write fake brain");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[brain]\ncommand = \"python3\"\nargs = [{}]\n",
            serde_json::to_string(&fake.to_string_lossy()).expect("quote fake path")
        ),
    )
    .expect("write config");
    config
}

/// 量的是行為：暫停中，畫面上的字**確實**還會送出去、還會變成新的一列。
///
/// 這一條在修這個 bug 之前就是綠的，而且**要一直綠**。它不是在守一個修好的
/// 東西，它是在守下面那條測試引用的那個事實——哪天有人真的讓暫停也擋住外送，
/// 這一條會紅，而那正是「句子該重寫」的訊號。
#[test]
fn pause_does_not_stop_the_brain_from_sending_or_writing() {
    let dir = seeded("pause-brain-writes");

    // 第一輪：大腦回空的，目的只是讓 segment 被算出來。
    let config = fake_brain(&dir, None);
    let _ = sister(&dir, Some(&config), &["interpret", "--last", "24h"]);

    let (core, frame) = {
        use sister_core::config::Config;
        use sister_core::db::Db;
        let db = Db::open(&Config::db_path(&dir)).expect("open db");
        let core: i64 = db
            .conn()
            .query_row(
                "SELECT core_started_at FROM segment ORDER BY core_started_at LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("一段都沒算出來，這條測試的前提就不成立");
        let frame: i64 = db
            .conn()
            .query_row(
                "SELECT frame_id FROM facts WHERE raw LIKE '%E0308%' AND frame_id IS NOT NULL LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("沒有畫面出處");
        (core, frame)
    };
    let config = fake_brain(&dir, Some((core, frame)));

    let before = counts(&dir);
    success(&dir, None, &["pause"]);
    let stdout = success(&dir, Some(&config), &["interpret", "--last", "24h"]);
    let after = counts(&dir);

    assert!(
        after.0 > before.0,
        "暫停中沒有任何東西送出去，那上面那句話就該重寫（外送 {} → {}）\n{stdout}",
        before.0,
        after.0
    );
    assert!(
        after.1 > before.1,
        "暫停中沒有寫成新卡片，那上面那句話就該重寫（卡片 {} → {}）\n{stdout}",
        before.1,
        after.1
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 讀的是句子：`sister doctor` 那一列不准說「什麼都不會被記錄」，而且要把
/// 上面那條測試量到的那一半講出來，還要給得出下一步。
#[test]
fn the_paused_row_says_which_half_stopped_and_names_the_lever_for_the_other() {
    let dir = seeded("pause-row-sentence");
    success(&dir, None, &["pause"]);

    let doctor = sister(&dir, None, &["doctor"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&doctor.stdout),
        String::from_utf8_lossy(&doctor.stderr)
    );
    let row = text
        .lines()
        .find(|l| l.contains("**暫停中**"))
        .unwrap_or_else(|| panic!("doctor 沒有印出暫停那一列：\n{text}"))
        .to_owned();

    assert!(
        !row.contains(THE_OLD_LIE),
        "暫停那一列還在說「{THE_OLD_LIE}」，而 interpret 在暫停中送得出去也寫得進去：\n{row}"
    );
    assert!(
        row.contains(STOPPED),
        "少了「她停了什麼」那半（要有「{STOPPED}」）：\n{row}"
    );
    assert!(
        row.contains(NOT_STOPPED),
        "少了「她沒停什麼」那半（要有「{NOT_STOPPED}」）：\n{row}"
    );
    assert!(
        row.contains(THE_LEVER),
        "說了壞消息卻沒給下一步（要有「{THE_LEVER}」）：\n{row}"
    );
    // 舊句子承諾的兩件事不准被順手刪掉。
    assert!(row.contains("resume"), "解除暫停那半不見了：\n{row}");
    assert!(row.contains("paused.flag"), "旗標路徑不見了：\n{row}");

    // 而它給的那個下一步要真的是一個跑得起來的指令，不是一句文案。
    let revoked = success(&dir, None, &["consent", "--revoke", "cloud-reading"]);
    assert!(
        !revoked.contains("上雲解讀") || !revoked.contains("✓ 我同意把"),
        "撤銷之後第二張同意書還打著勾：\n{revoked}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
