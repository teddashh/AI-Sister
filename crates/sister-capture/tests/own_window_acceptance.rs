//! A158 驗收（Claude 在看到 delegate 的 diff 之前寫的）：端到端。
//!
//! 跑一段腳本，她自己的三個身分各出現一段，畫面、標題、剪貼簿都放獨有的哨兵字串；
//! 然後把資料目錄整個當位元組掃一遍（同 `privacy.rs` 的做法）。

use sister_capture::replay::{
    ReplayBackend, ReplayPrivacyContext, ReplaySystemState, Scenario, Step,
};
use sister_capture::{MasterStopSource, Recorder, Tick};
use sister_core::config::Config;
use sister_core::db::Db;

const MUST_NEVER_APPEAR: &[&str] = &[
    "A158-SENTINEL-WIN",
    "A158-SENTINEL-TITLE",
    "A158-SENTINEL-CLIP-WIN",
    "A158-SENTINEL-LINUX",
    "A158-SENTINEL-MAC",
    "A158-SENTINEL-CLIP-SRC",
    "A158-SENTINEL-CLIP-WEBVIEW",
    "A158-SENTINEL-CLIP-TAIL",
    "own window",
];
const MUST_APPEAR: &[&str] = &[
    "0800-080-123",
    "A158-KEEP-AFTER",
    "A158-KEEP-END",
    "A158-NEARMISS-MUST-BE-KEPT",
];

fn at(at_ms: i64, app: &str, title: &str, text: &[&str]) -> Step {
    Step {
        at_ms,
        app: Some(app.into()),
        title: Some(title.into()),
        text: text.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn scenario() -> Scenario {
    let mut her_win = at(
        4_000,
        "sister-desktop.exe",
        "A158-SENTINEL-TITLE AI-Sister",
        &["A158-SENTINEL-WIN 我最後看到的是 0800-080-123"],
    );
    her_win.clipboard = Some("A158-SENTINEL-CLIP-WIN".into());
    her_win.clipboard_source_app = Some("sister-desktop.exe".into());
    let mut copied_from_her = at(
        9_000,
        "code.exe",
        "main.rs - project",
        &["fn main() {} A158-KEEP-AFTER"],
    );
    copied_from_her.clipboard = Some("A158-SENTINEL-CLIP-SRC".into());
    copied_from_her.clipboard_source_app = Some("sister-desktop.exe".into());
    let mut her_mac = at(
        12_000,
        "com.ted-h.ai-sister",
        "AI-Sister",
        &["A158-SENTINEL-MAC"],
    );
    // 在她前景時複製、而剪貼簿擁有者不是她的程式檔（WebView2 的複製可能記在
    // msedgewebview2.exe 名下）：來源閘門認不出她，只剩前景那一拍的水位擋得住。
    // 它是下一個一般 tick 前最後一筆，水位沒推過去就會被讀進來。
    her_mac.clipboard = Some("A158-SENTINEL-CLIP-WEBVIEW".into());
    her_mac.clipboard_source_app = Some("msedgewebview2.exe".into());
    // 最後一個她的 tick（13 s）之後、下一個一般 tick（14 s）之前才複製：
    // 那一拍的水位已經過去了，只剩離開她時的空洞旗標擋得住這條尾巴。
    let mut her_tail = at(13_500, "com.ted-h.ai-sister", "AI-Sister", &[]);
    her_tail.clipboard = Some("A158-SENTINEL-CLIP-TAIL".into());
    her_tail.clipboard_source_app = Some("msedgewebview2.exe".into());
    Scenario {
        name: "a158-own-window".into(),
        privacy_context: ReplayPrivacyContext::Clear,
        system_state: ReplaySystemState::Active,
        steps: vec![
            // 不用瀏覽器：預設有網址規則，瀏覽器沒給網址會以「browser URL state unknown」擋掉。
            at(0, "notepad.exe", "帳單.txt", &["客服專線 0800-080-123"]),
            at(2_000, "keepassxc.exe", "KeePassXC", &["vault"]),
            her_win,
            at(6_000, "keepassxc.exe", "KeePassXC", &["vault"]),
            at(
                8_000,
                "code.exe",
                "main.rs - project",
                &["fn main() {} A158-KEEP-AFTER"],
            ),
            copied_from_her,
            at(
                10_000,
                "sister-desktop",
                "AI-Sister",
                &["A158-SENTINEL-LINUX"],
            ),
            her_mac,
            her_tail,
            at(
                14_000,
                "code.exe",
                "main.rs - project",
                &["fn main() {} A158-KEEP-END"],
            ),
            at(
                16_000,
                "my-sister-desktop.exe",
                "not her",
                &["A158-NEARMISS-MUST-BE-KEPT"],
            ),
        ],
    }
}

fn all_bytes(dir: &std::path::Path, out: &mut Vec<u8>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            all_bytes(&p, out);
        } else if let Ok(b) = std::fs::read(&p) {
            out.extend_from_slice(&b);
        }
    }
}

fn contains(h: &[u8], n: &str) -> bool {
    h.windows(n.len()).any(|w| w == n.as_bytes())
}

#[test]
fn her_own_window_never_reaches_the_disk_and_is_not_counted_as_his_rule() {
    let dir = std::env::temp_dir().join(format!("sister-a158-own-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("sister.db")).unwrap();
    let mut rec = Recorder::new(
        ReplayBackend::new(scenario()),
        db,
        Config::default(),
        Some(dir.join("frames")),
        MasterStopSource::NotApplicable,
    )
    .unwrap();

    let mut own_ticks = Vec::new();
    let mut excluded_ticks = 0u64;
    for ts in (0..=17_000).step_by(1_000) {
        match rec.tick(ts) {
            Ok(Tick::OwnWindow) => own_ticks.push(ts),
            Ok(Tick::Excluded { reason }) => {
                assert!(
                    !reason.contains("sister") && !reason.contains("own window"),
                    "tick {ts}：她的視窗被當成排除規則：{reason}"
                );
                excluded_ticks += 1;
            }
            Ok(_) => {}
            Err(e) => panic!("tick {ts} failed: {e:#}"),
        }
    }
    rec.finish(sister_core::model::EndReason::Duration).unwrap();
    let stats = rec.stats().clone();
    let db = rec.into_db();
    let audit = db.exclusion_audit().unwrap();
    drop(db);

    // 焦點換到她身上的那三拍一定會看（焦點變了），所以一定是 OwnWindow。
    for ts in [4_000, 10_000, 12_000] {
        assert!(
            own_ticks.contains(&ts),
            "tick {ts} 應該是 OwnWindow；實際 OwnWindow 的拍：{own_ticks:?}"
        );
    }
    assert_eq!(
        stats.own_window,
        own_ticks.len() as u64,
        "own_window 要等於真的回 OwnWindow 的拍數"
    );
    assert_eq!(
        stats.excluded, excluded_ticks,
        "excluded 只數真的回 Excluded 的拍"
    );
    assert!(
        excluded_ticks >= 2,
        "前提：KeePassXC 兩段都有被擋到：{excluded_ticks}"
    );
    assert!(
        stats
            .excluded_reasons
            .keys()
            .all(|r| r.contains("keepassxc")),
        "排除理由裡只該有 KeePassXC：{:?}",
        stats.excluded_reasons
    );
    assert!(
        stats.clipboard_source_own_window >= 1,
        "從她視窗複製、在 code.exe 那拍才讀到的那筆要算在她身上"
    );
    assert_eq!(stats.clipboard_source_excluded, 0, "她的剪貼簿不算排除規則");

    assert_eq!(audit.len(), 1, "稽核只該有 KeePassXC 一種理由：{audit:?}");
    assert!(audit[0].reason.contains("keepassxc"), "{audit:?}");
    assert_eq!(
        audit[0].episodes, 2,
        "她的視窗夾在中間，KeePassXC 是兩段：{audit:?}"
    );

    let mut disk = Vec::new();
    all_bytes(&dir, &mut disk);
    assert!(!disk.is_empty());
    let leaked: Vec<_> = MUST_NEVER_APPEAR
        .iter()
        .filter(|n| contains(&disk, n))
        .collect();
    assert!(leaked.is_empty(), "這些出現在磁碟上：{leaked:?}");
    let missing: Vec<_> = MUST_APPEAR.iter().filter(|n| !contains(&disk, n)).collect();
    assert!(missing.is_empty(), "這些該記下來卻沒有：{missing:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
