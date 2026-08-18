//! 敏感排除的端到端驗證（PHASES.md Phase 0 exit criteria）。
//!
//! 單元測試證明的是「排除函式回傳 Blocked」。這裡要證明的是更強的一件事：
//! **那些內容從來沒有進到磁碟上的任何地方。**
//!
//! 做法是跑完一段包含密碼管理員、網銀、螢幕分享、剪貼簿秘密的腳本，
//! 然後把整個資料庫**當成位元組**掃一遍，找那些不該存在的字串。這樣掃
//! 的好處是它不依賴我想得到要檢查哪些欄位——多一個欄位、多一張表、
//! 未來多一個索引，只要內容漏出去就會被抓到。

use sister_capture::replay::{ReplayBackend, Scenario, Step};
use sister_capture::{Recorder, Tick};
use sister_core::config::Config;
use sister_core::db::Db;

/// 絕對不可以出現在磁碟上的字串。
const MUST_NEVER_APPEAR: &[(&str, &str)] = &[
    ("hunter2-my-real-banking-password", "密碼管理員畫面上的密碼"),
    (
        "Vault: Cathay Bank / ted@example.com",
        "密碼管理員的條目標題",
    ),
    ("sk-proj-DEADBEEF0123456789abcdefXYZ", "剪貼簿裡的 API key"),
    ("轉出帳號 0123456789 餘額 1,204,338", "網銀畫面上的餘額"),
    ("Bob 的薪資是 120,000", "螢幕分享時看到的別人畫面"),
    (
        "correct horse battery staple",
        "在被排除的 app 裡複製的通行短語",
    ),
];

fn step(at_ms: i64, app: &str, title: &str, text: &[&str]) -> Step {
    Step {
        at_ms,
        app: Some(app.into()),
        title: Some(title.into()),
        text: text.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// 一段刻意踩滿地雷的工作日切片。
fn minefield() -> Scenario {
    Scenario {
        name: "privacy-minefield".into(),
        steps: vec![
            // 正常工作：這些**應該**被記下來，用來確認我們不是靠「什麼都不記」過關
            step(
                0,
                "chrome.exe",
                "中華電信 帳單查詢",
                &["本期應繳 NT$13,450", "客服專線 0800-080-123"],
            ),
            // 密碼管理員：整段不該存在
            step(
                4_000,
                "keepassxc.exe",
                "KeePassXC",
                &[
                    "Vault: Cathay Bank / ted@example.com",
                    "hunter2-my-real-banking-password",
                ],
            ),
            // 在被排除的 app 裡複製東西 → 連剪貼簿都不該碰
            Step {
                at_ms: 6_000,
                app: Some("keepassxc.exe".into()),
                title: Some("KeePassXC".into()),
                clipboard: Some("correct horse battery staple".into()),
                clipboard_source_app: Some("keepassxc.exe".into()),
                ..Default::default()
            },
            // 網銀：關鍵字在網域裡，不在路徑裡——這是真的漏過一次的形狀
            Step {
                at_ms: 8_000,
                app: Some("chrome.exe".into()),
                title: Some("網路銀行".into()),
                url: Some("https://netbank.cathaybk.com.tw/transfer".into()),
                text: vec!["轉出帳號 0123456789 餘額 1,204,338".into()],
                ..Default::default()
            },
            // 螢幕分享：看到的是別人的畫面，不是她的
            step(
                12_000,
                "zoom.exe",
                "Zoom Meeting",
                &["Bob 的薪資是 120,000"],
            ),
            // 視窗標題含 password
            step(
                16_000,
                "chrome.exe",
                "Change your password - Example",
                &["新密碼", "確認新密碼"],
            ),
            // 回到正常工作，並複製一把 API key（允許的 app，但內容是秘密）
            Step {
                at_ms: 20_000,
                app: Some("code.exe".into()),
                title: Some("main.rs - project".into()),
                text: vec!["fn main() {}".into()],
                clipboard: Some("sk-proj-DEADBEEF0123456789abcdefXYZ".into()),
                clipboard_source_app: Some("code.exe".into()),
                ..Default::default()
            },
        ],
    }
}

struct Run {
    dir: std::path::PathBuf,
    kept: u64,
    excluded: u64,
    secrets_redacted: u64,
    clipboard_events: u64,
}

fn run_minefield(name: &str) -> Run {
    // 每個測試自己的乾淨目錄。測試是平行跑的，共用一個目錄會互相搶鎖。
    let dir = std::env::temp_dir().join(format!("sister-privacy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");

    let db = Db::open(&dir.join("sister.db")).expect("open db");
    let backend = ReplayBackend::new(minefield());
    // 預設設定：使用者不必自己設定任何東西就該是安全的
    let config = Config::default();
    let mut rec = Recorder::new(backend, db, config, Some(dir.join("frames"))).expect("recorder");

    // 每秒一格跑完整段腳本
    for ts in (0..=22_000).step_by(1_000) {
        match rec.tick(ts) {
            Ok(Tick::Kept { .. }) | Ok(_) => {}
            Err(e) => panic!("tick {ts} failed: {e:#}"),
        }
    }
    rec.finish().expect("finish");
    let stats = rec.stats().clone();
    // 收掉連線，讓 WAL 落回主檔
    drop(rec.into_db());

    Run {
        dir,
        kept: stats.kept,
        excluded: stats.excluded,
        secrets_redacted: stats.secrets_redacted,
        clipboard_events: stats.clipboard_events,
    }
}

/// 把資料目錄下所有檔案的位元組串起來（含 -wal / -shm 與畫面檔）。
fn all_bytes_on_disk(dir: &std::path::Path) -> Vec<u8> {
    fn walk(dir: &std::path::Path, out: &mut Vec<u8>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if let Ok(bytes) = std::fs::read(&p) {
                out.extend_from_slice(&bytes);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

/// 這是 Phase 0 的驗收條件本身：敏感內容不得落地。
#[test]
fn sensitive_content_never_reaches_the_disk() {
    let run = run_minefield("nothing-leaks");
    let disk = all_bytes_on_disk(&run.dir);
    assert!(!disk.is_empty(), "什麼都沒寫出來，這個測試就沒有意義");

    let mut leaked = Vec::new();
    for (needle, what) in MUST_NEVER_APPEAR {
        if contains(&disk, needle) {
            leaked.push(format!("  ✗ {what}：「{needle}」"));
        }
    }
    assert!(
        leaked.is_empty(),
        "以下內容出現在磁碟上：\n{}",
        leaked.join("\n")
    );

    let _ = std::fs::remove_dir_all(&run.dir);
}

/// 反面對照：正常工作的內容**必須**被記下來。
///
/// 沒有這一條的話，「什麼都不記」就能讓上面那個測試變成綠的，
/// 而那是一個完全無用卻看起來很安全的產品。
#[test]
fn ordinary_work_is_still_remembered() {
    let run = run_minefield("still-remembers");
    let disk = all_bytes_on_disk(&run.dir);

    for needle in ["0800-080-123", "13,450"] {
        assert!(
            contains(&disk, needle),
            "正常工作的內容「{needle}」沒有被記下來——排除規則過寬"
        );
    }
    assert!(run.kept > 0, "一張畫面都沒留下");
    assert!(run.excluded > 0, "沒有任何東西被排除，腳本沒踩到地雷");

    let _ = std::fs::remove_dir_all(&run.dir);
}

/// 剪貼簿的秘密要留下「發生過」的痕跡，但不留內容。
///
/// 兩件事都重要：她要能想起「我那時候複製了一把 key」，
/// 但那把 key 本身不該在這裡。
#[test]
fn clipboard_secrets_are_recorded_as_events_without_content() {
    let run = run_minefield("clipboard");
    assert_eq!(run.secrets_redacted, 1, "應該正好擋下一把 API key");
    assert_eq!(
        run.clipboard_events, 1,
        "被排除的 app 裡複製的那一次不該產生事件，允許的那一次要"
    );
    let _ = std::fs::remove_dir_all(&run.dir);
}
