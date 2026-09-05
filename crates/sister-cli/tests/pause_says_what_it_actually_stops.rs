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
//! **這一檔的兩條測試要一起讀**：一條量行為、一條讀句子。它們各自開一個資料
//! 目錄（名字不同，互不干擾），綁在一起的是**那句話**：行為那條量到的東西，
//! 就是句子那條要求講出來的東西。分開寫的話，句子改對了而行為變了（或反過來）
//! 不會有人紅。
//!
//! **這一檔蓋不到 Windows 那半。** `pause_warning`（record 迴圈開頭那句）是
//! `#[cfg(any(windows, test))]`，唯一的呼叫端在 `#[cfg(windows)] fn
//! windows_record`——在 Linux 上它根本沒被編進 `CARGO_BIN_EXE_sister`，整合
//! 測試碰不到。守它的是 `ops::record::pause_warning_tests` 那條單元測試；我拿
//! 突變驗過它有牙齒（把那句話改回舊的假話，那條會紅）。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 這一版承諾的那句話，逐字。分成兩半是因為它們證明的是不同的事：
/// 前半是「她停了什麼」，後半是「她沒停什麼」。少任何一半都是舊 bug。
const STOPPED: &str = "她不會再看新的畫面";
const NOT_STOPPED: &str = "已經記下來的";
/// 上面兩個都是**名詞片語**，證明不了極性——把整句話的意思反過來寫成「而且
/// **已經記下來的**那些也一併停了：解釋層不會讀、不會送給雲端模型」，兩個針
/// 照樣命中，而它說的正是這一版證明為假的那句話。所以要有一個**帶著動詞**的
/// 針：句子必須承認那半還在送。
const STILL_SENDS: &str = "還是可能送給雲端模型";
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

/// 回傳（送出去的**字數**總和、卡片數）。
///
/// 第一格刻意不是 `list_brain_outbound(..).len()`：`brain.rs` 連「叫不起 CLI」
/// 也會寫一列稽核，`chars_sent` 是 0。數列數的話，一台沒有 `python3` 的機器
/// 會讓「暫停中真的送了東西出去」這條斷言**通過**，然後在下一條卡片斷言上紅
/// 掉，訊息指向錯的地方。要證明「字離開了這台機器」就得數字。
fn counts(dir: &Path) -> (usize, usize) {
    use sister_core::config::Config;
    use sister_core::db::Db;
    let db = Db::open(&Config::db_path(dir)).expect("open db");
    let outbound: usize = db
        .list_brain_outbound(1000)
        .expect("outbound")
        .iter()
        .map(|r| r.chars_sent.max(0) as usize)
        .sum();
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
        "暫停中一個字都沒送出去，那上面那句話就該重寫（送出字數 {} → {}）\n{stdout}",
        before.0,
        after.0
    );
    assert!(
        after.1 > before.1,
        "暫停中沒有寫成新卡片，那上面那句話就該重寫（卡片 {} → {}）\n{stdout}",
        before.1,
        after.1
    );

    // 而句子那條測試要求印出來的那個**下一步**，得真的關得掉這半。這裡是唯一
    // 有大腦、有段落、有東西可送的地方，所以在這裡量：撤掉那張同意書之後，同
    // 一道 interpret 一個字都不准再送、一張卡片都不准再寫。
    //
    // 不在句子那條測試裡拿 `sister consent` 的輸出去比字串——我試過，那是一條
    // 死斷言：`上雲解讀` 這四個字只出現在 doctor 的隱私摘要裡，consent 自己
    // 從來不印，於是 `!contains` 恆真。**要證明一根拉桿有用，就去拉它。**
    success(&dir, None, &["consent", "--revoke", "cloud-reading"]);
    let stdout = success(&dir, Some(&config), &["interpret", "--last", "24h"]);
    let after_revoke = counts(&dir);
    assert_eq!(
        after_revoke, after,
        "撤掉上雲同意書之後還在送／還在寫，那 doctor 給的那個下一步就是假的\n{stdout}"
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

    // 這一條問**整份輸出**不是那一列。`lines().find()` 只會拿到第一行含
    // 「**暫停中**」的，所以「在上面補一句好聽的、底下那句假話原封不動」會
    // 全綠。實測現在整份 doctor 裡這句話出現 0 次，收緊不會假紅。
    assert!(
        !text.contains(THE_OLD_LIE),
        "還有地方在說「{THE_OLD_LIE}」，而 interpret 在暫停中送得出去也寫得進去：\n{text}"
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
        row.contains(STILL_SENDS),
        "「已經記下來的」那半沒有承認它還在送（要有「{STILL_SENDS}」）：\n{row}"
    );
    assert!(
        row.contains(THE_LEVER),
        "說了壞消息卻沒給下一步（要有「{THE_LEVER}」）：\n{row}"
    );
    // 舊句子承諾的兩件事不准被順手刪掉。
    assert!(row.contains("resume"), "解除暫停那半不見了：\n{row}");
    assert!(row.contains("paused.flag"), "旗標路徑不見了：\n{row}");

    // **兩個指令要各自待在自己的槽裡。** 上面那四條 `contains` 全部只問「這幾
    // 個字在不在這一行」，而這一句話裡有兩個反引號指令。我拿突變實測過：把
    // `format!` 的兩個引數**對調**——句子變成「要連那半也停，請跑 `sister
    // resume`。解除暫停請跑 `sister consent --revoke cloud-reading`」——六個
    // test binary 全綠。兩句話都還在，只是各自指到對方，而照著做的人會先把
    // 記錄接回去、再把上雲同意書撤掉，兩件事都跟他想做的相反。
    //
    // 針要含著它所屬的那句承諾，不能只含指令本身。
    assert_eq!(
        command_in_slot(&row, "要連那半也停，請跑 `"),
        Some(THE_LEVER),
        "「要連那半也停」後面那個指令不是關外送的那一個：\n{row}"
    );
    assert_eq!(
        command_in_slot(&row, "解除暫停請跑 `"),
        Some("resume"),
        "「解除暫停請跑」後面那個指令不是 resume：\n{row}"
    );

    // 「這個下一步真的關得掉那半」由上面那條行為測試證明（撤掉之後同一道
    // interpret 一個字都不再送）。這裡不再拿 consent 的輸出比字串。

    let _ = std::fs::remove_dir_all(&dir);
}

/// 從一句話裡把「`marker` 後面那一對反引號之間的指令」挖出來，再回傳它**尾巴**
/// 的那一段（`sister --data-dir /長/路/徑 ` 之後）。
///
/// 回傳 `&'static str` 是刻意的：呼叫端只拿它跟兩個已知常數比對，而
/// `assert_eq!` 的失敗訊息會把整行印出來，所以不需要把實際值帶回去。
fn command_in_slot(row: &str, marker: &str) -> Option<&'static str> {
    let start = row.find(marker)? + marker.len();
    let rest = &row[start..];
    let cmd = &rest[..rest.find('`')?];
    // 只認得這兩個；認不出來就回 None，讓 assert_eq! 紅在「不是預期那個」上。
    [THE_LEVER, "resume"]
        .into_iter()
        .find(|known| cmd.ends_with(known))
}
