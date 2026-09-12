//! `sister diagnose` 讀的是已經在磁碟上的東西，而且不會把原文帶出去。
//!
//! 這裡跑的是真的執行檔（`CARGO_BIN_EXE_sister`），不是 helper——`ops::diagnose`
//! 那一層的工作就是「把哪些檔案、哪幾張表接到 `render` 上」，而那件事只有整支
//! 跑起來才證得了。版面與遮蔽本身在 `sister_core::diagnose` 有自己的單元測試。

use sister_core::config::Config;
use sister_core::db::{Db, OutboundInsert};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 螢幕上會有、報告裡絕對不該有的一串字。
const SCREEN_TEXT: &str = "王小明的帳號密碼是 hunter2";
/// `brain_skip` 的 `detail` 存的是那句會跟著文案改的話。報告只該印代號。
const SKIP_DETAIL: &str = "最新一段還沒有假設；第二張同意書尚未簽署，解釋層一次都不會呼叫 CLI。";

fn sister(data_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sister"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("run sister")
}

fn success(data_dir: &Path, args: &[&str]) -> String {
    let output = sister(data_dir, args);
    assert!(
        output.status.success(),
        "sister {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 stdout")
}

fn fresh(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sister-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

fn seed_two_legs(dir: &Path) {
    let mut db = Db::open(&Config::db_path(dir)).expect("open db");
    // 一題兩趟：先問它要查什麼，再請它把查到的寫成一句。
    db.insert_brain_outbound(&OutboundInsert {
        ts: 1_789_222_440_000,
        day_key: "2026-09-12",
        command: "sister",
        args: &[],
        segment_core_start: None,
        chars_sent: 418,
        truncated: false,
        outcome: "success",
        duration_ms: 1_204,
        error: None,
        role: "answer_search",
    })
    .expect("seed search leg");
    db.insert_brain_outbound(&OutboundInsert {
        ts: 1_789_222_441_000,
        day_key: "2026-09-12",
        command: "sister",
        args: &[],
        segment_core_start: None,
        chars_sent: 12_004,
        truncated: false,
        outcome: "bad_json",
        duration_ms: 3_118,
        // serde 的 `invalid type` 會把模型吐回來的字整段帶進錯誤訊息裡。
        // 這正是 `error` 原文不能印的理由。
        error: Some(&format!(
            "JSON 對不上回答契約：invalid type: string \"{SCREEN_TEXT}\", expected a sequence"
        )),
        role: "answer",
    })
    .expect("seed answer leg");
    db.insert_brain_skip(1_789_222_000_000, "no_consent", None, SKIP_DETAIL)
        .expect("seed skip");
    drop(db);
}

#[test]
fn the_report_reads_the_audit_that_was_already_on_disk() {
    let dir = fresh("diagnose-reads-disk");
    seed_two_legs(&dir);
    // tracing 寫進去的東西會帶著完整路徑。
    std::fs::write(
        dir.join("desktop.log"),
        format!(
            "2026-09-12T20:00:02Z WARN sister_desktop: 讀不到 {}/sister.db\n",
            dir.display()
        ),
    )
    .expect("write desktop.log");

    let out = dir.join("report.txt");
    success(
        &dir,
        &["diagnose", "--out", out.to_str().expect("out path")],
    );
    let text = std::fs::read_to_string(&out).expect("read report");

    // 兩趟各自看得見，而且分得出來是哪一趟。
    assert!(text.contains("answer_search"), "{text}");
    assert!(text.contains("1,204"), "{text}");
    assert!(text.contains("3,118"), "{text}");
    assert!(
        text.contains("先問它要查什麼   1 趟，中位數 1,204 毫秒"),
        "{text}"
    );
    assert!(
        text.contains("再請它寫成一句   1 趟，中位數 3,118 毫秒"),
        "{text}"
    );
    assert!(text.contains("兩個中位數加起來 4,322 毫秒"), "{text}");

    // 錯誤只剩類別和字數。
    assert!(text.contains("JSON 對不上契約"), "{text}");
    assert!(
        !text.contains(SCREEN_TEXT),
        "外送紀錄的錯誤原文漏進報告了：\n{text}"
    );
    assert!(!text.contains("invalid type"), "{text}");

    // 跳過的理由只剩代號。
    assert!(text.contains("no_consent"), "{text}");
    assert!(
        !text.contains("第二張同意書尚未簽署"),
        "`detail` 漏進報告了：\n{text}"
    );

    // log 尾巴讀到了，而且路徑被換掉了。
    assert!(text.contains("desktop.log（一共 1 行"), "{text}");
    assert!(text.contains("<資料夾>/sister.db"), "{text}");
    assert!(
        !text.contains(&dir.display().to_string()),
        "資料夾路徑漏進報告了：\n{text}"
    );
}

/// 打錯一個字就把別的東西蓋掉，是這種「隨手跑一下」的指令最容易造成的損失。
#[test]
fn it_refuses_to_overwrite_something_that_is_not_a_report() {
    let dir = fresh("diagnose-refuses-clobber");
    let precious = dir.join("thesis.txt");
    std::fs::write(&precious, "我的畢業論文").expect("write thesis");

    let output = sister(
        &dir,
        &["diagnose", "--out", precious.to_str().expect("path")],
    );
    assert!(!output.status.success(), "應該要拒絕");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("我不覆蓋它"),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&precious).expect("read thesis"),
        "我的畢業論文",
        "拒絕之後那個檔一個位元組都不該動"
    );

    // 但重跑是正常流程：上一份報告可以蓋掉。
    let out = dir.join("report.txt");
    let path = out.to_str().expect("out path");
    success(&dir, &["diagnose", "--out", path]);
    success(&dir, &["diagnose", "--out", path]);
    assert!(
        std::fs::read_to_string(&out)
            .expect("read report")
            .starts_with("AI-Sister 診斷報告"),
        "第二次跑完還是要是一份報告"
    );
}

/// 資料庫還沒有的時候也要跑得出東西來，而且每一節都要說出自己為什麼是空的。
#[test]
fn a_machine_with_nothing_on_it_still_gets_a_report_that_says_why() {
    let dir = fresh("diagnose-empty-machine");
    let out = dir.join("report.txt");
    success(
        &dir,
        &["diagnose", "--out", out.to_str().expect("out path")],
    );
    let text = std::fs::read_to_string(&out).expect("read report");

    for heading in [
        "① 這台機器",
        "② 她問 CLI 的每一趟",
        "③ 自檢",
        "④ 這份報告帶走了什麼",
        "⑤ 她最後幾句答案的原文",
        "⑥ log 尾巴",
    ] {
        assert!(text.contains(heading), "{heading} 整節不見了：\n{text}");
    }
    assert!(
        text.contains("沒有這個檔案"),
        "空的那幾格要說出理由：\n{text}"
    );
    assert!(
        text.contains("這不是「沒問題」，是「沒量」"),
        "自檢那一節不可以讀起來像通過了：\n{text}"
    );
}

/// 不給 `--out` 的時候，檔名帶時間，寫在目前的資料夾。
#[test]
fn without_an_out_path_it_names_the_file_after_the_clock() {
    let dir = fresh("diagnose-default-name");
    let cwd = dir.join("cwd");
    std::fs::create_dir_all(&cwd).expect("create cwd");
    let output = Command::new(env!("CARGO_BIN_EXE_sister"))
        .arg("--data-dir")
        .arg(&dir)
        .args(["diagnose"])
        .current_dir(&cwd)
        .output()
        .expect("run sister");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let written: Vec<PathBuf> = std::fs::read_dir(&cwd)
        .expect("list cwd")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    assert_eq!(written.len(), 1, "{written:?}");
    let name = written[0]
        .file_name()
        .expect("name")
        .to_string_lossy()
        .to_string();
    assert!(name.starts_with("sister-diagnose-"), "{name}");
    assert!(name.ends_with(".txt"), "{name}");
    assert!(
        std::fs::read_to_string(&written[0])
            .expect("read report")
            .starts_with("AI-Sister 診斷報告"),
        "{name}"
    );
}
