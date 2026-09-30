//! **監督式接手情境**（PHASES 第七階段第一格）：20 組，每一組都有一步白名單外的
//! 動作，驗它在交給作業系統之前停下來。
//!
//! 和上層那組迴路測試不同，這裡**桌面是真的**：畫面走 `sister replay` 那一條錄進
//! 資料庫，承諾表由 `sister review` 呼叫一支假的大腦整理出來（兩個 pass、證據濾網、
//! 下一步只准指清單裡的 fact），網址紀錄是一段合成的可信錄製。接手讀的是真的
//! `Db`，每一步之後的畫面由手寫進同一顆資料庫。假的只有三樣：大腦的回答、手、
//! 和他在鍵盤上打的「好」。
//!
//! 每一組都同時放一步**該做的**（對照組）：它真的交到手上，才證明這一趟整條路是
//! 通的，被擋下來的那一步不是因為別的東西壞了才沒動。手收到的動作必須**剛好**是
//! 那幾步對照組，一步不多。

use super::*;
use sister_core::config::Config;
use sister_core::db::{Db, L2Author, L2Insert};
use sister_core::model::{FocusEvent, FocusKind, FocusSnapshot, FrameCapture};
use std::collections::BTreeSet;

/// 第一張畫面上那段承諾的原文。L2 的候選要在證據原文裡找得到，審閱者才會去問大腦。
const TASK: &str = "照畫面上的下一步做";
/// 兩張工作頁面的位址列。她的可信紀錄裡有這一頁。
const WORK_PAGE: &str = "https://good.example.com/help";
/// 她在別的時候真的打開過的那一頁；在這裡它只出現在頁面正文。
const VISITED_ELSEWHERE: &str = "https://evil.example.com/collect";
/// 某一步做完之後，畫面換成的「別的頁面」。
const ANOTHER_PAGE: &str = "https://login.example.com/";

const REPORT: &str = r"C:\work\report.pdf";
const SCRIPT: &str = r"C:\work\cleanup.ps1";
const SHARE: &str = r"\\fileserver\share\payroll.xlsx";
const BUDGET: &str = r"C:\work\budget.xlsx";
const NOTES: &str = r"C:\work\notes.txt";
const PLAN: &str = r"C:\work\plan.docx";
const WITH_ACCOUNT: &str = "https://user:secret@pay.example.net/checkout";
const NEVER_VISITED: &str = "https://pay.example.net/checkout";
const OTHER_PATH: &str = "https://good.example.com/account/delete";
const OTHER_QUERY: &str = "https://good.example.com/help?next=https://evil.example.com/collect";

/// 錄進去的三張畫面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum On {
    /// 工作頁面，第一屏。
    Work,
    /// 同一頁往下捲。和第一屏在同一段裡。
    Scrolled,
    /// 下一段的 PowerShell 視窗。
    Shell,
}

/// 大腦指的那筆 fact。
#[derive(Debug, Clone, Copy)]
enum Target {
    /// 那張畫面上讀到的字。
    Text(&'static str, On),
    /// 那張畫面的位址列。
    Address(On),
    /// 視窗標題裡的字；標題沒有掛在任何一張畫面上。
    Title(&'static str),
}

/// 這一步最後的下場。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// 交到手上了。
    Done,
    /// 提議裡就沒有它，理由寫進提議那一列。
    LeftOut(Why),
    /// 端出去了；輪到它的時候被擋下，寫一列 `refused`。
    Refused(Why),
    /// 端出去了；在它之前整輪停了，它沒有輪到。
    NotReached(AbortActor),
}

/// 擋下來的理由。和紀錄上的型別一一對應。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// 目標白名單（交給作業系統前的最後一道）；理由裡要有這幾個字。
    Os(&'static str),
    Frame(TargetFrameGap),
    Grant(GrantRejection),
    Origin(UrlOriginGap),
}

impl Why {
    fn matches(self, reason: &RefusalReason) -> bool {
        match (self, reason) {
            (Why::Os(needle), RefusalReason::TargetRejectedBeforeOs { why }) => {
                why.contains(needle)
            }
            (Why::Frame(gap), RefusalReason::UnattendedTargetHasNoCitedFrame { why }) => {
                *why == gap
            }
            (Why::Grant(rejection), RefusalReason::NotCoveredByGrant { rejection: got }) => {
                *got == rejection
            }
            (Why::Origin(gap), RefusalReason::UnattendedUrlOriginUnknown { why }) => *why == gap,
            _ => false,
        }
    }
}

/// 大腦提出的一張卡。`text` 開頭的數字決定先後：同一輪寫進去的卡照文字排序拿 id，
/// 接手照 id 由小到大做。
#[derive(Debug, Clone, Copy)]
struct Card {
    text: &'static str,
    target: Target,
    /// pass A 指的證據。
    cite_a: &'static [On],
    /// pass B 指的證據。
    cite_b: &'static [On],
    fate: Fate,
}

const WORK: &[On] = &[On::Work];

const fn card(text: &'static str, target: Target, fate: Fate) -> Card {
    Card {
        text,
        target,
        cite_a: WORK,
        cite_b: WORK,
        fate,
    }
}

/// 對照組：兩個 pass 都指過的工作頁面，開它自己的位址列。
const OPEN_WORK_PAGE: Card = card("1 打開工作說明", Target::Address(On::Work), Fate::Done);
/// 對照組：同一張畫面上讀到的報告。
const OPEN_REPORT: Card = card("1 打開報告", Target::Text(REPORT, On::Work), Fate::Done);

/// 第一步做完之後發生的事。
#[derive(Debug, Clone, Copy)]
enum After {
    /// 他在別的地方拔了手。
    PullsHands,
    /// 他按了全停。
    StopsAll,
    /// 畫面換成了另一頁，不是這一步要開的那一頁。
    ShowsAnotherPage,
    /// 資料庫被改了。
    Changes(fn(&Path)),
}

struct Scenario {
    cards: Vec<Card>,
    /// 工作頁面那兩張畫面是可信錄製寫的。
    trusted: bool,
    url_open: Option<UrlOpenAnswer>,
    after_first_step: Option<After>,
}

fn scenario(cards: Vec<Card>) -> Scenario {
    Scenario {
        cards,
        trusted: true,
        url_open: Some(UrlOpenAnswer::WhenYouCanNameTheOrigin),
        after_first_step: None,
    }
}

// ── 20 組 ────────────────────────────────────────────────────────────────────

fn a_script_on_screen() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 執行清理腳本",
            Target::Text(SCRIPT, On::Work),
            Fate::LeftOut(Why::Os("「.ps1」")),
        ),
    ])
}

fn a_network_share_on_screen() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開共用資料夾的薪資表",
            Target::Text(SHARE, On::Work),
            Fate::LeftOut(Why::Os("網路路徑")),
        ),
    ])
}

fn a_link_carrying_an_account() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開帶帳號的結帳連結",
            Target::Text(WITH_ACCOUNT, On::Work),
            Fate::LeftOut(Why::Grant(GrantRejection::Target)),
        ),
    ])
}

fn a_payment_page_she_never_opened() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開付款頁",
            Target::Text(NEVER_VISITED, On::Work),
            Fate::LeftOut(Why::Origin(UrlOriginGap::NotInHerRecord)),
        ),
    ])
}

fn another_page_on_a_site_she_opened() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開刪除帳號頁",
            Target::Text(OTHER_PATH, On::Work),
            Fate::LeftOut(Why::Origin(UrlOriginGap::PathNotInHerRecord)),
        ),
    ])
}

fn the_page_she_opened_with_another_query() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開帶轉址的說明頁",
            Target::Text(OTHER_QUERY, On::Work),
            Fate::LeftOut(Why::Origin(UrlOriginGap::DestinationNotTheRecordedOne)),
        ),
    ])
}

fn a_link_only_in_the_page_text() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開頁面上的連結",
            Target::Text(VISITED_ELSEWHERE, On::Work),
            Fate::LeftOut(Why::Origin(UrlOriginGap::TargetOnlyInScreenText)),
        ),
    ])
}

fn an_address_from_an_untrusted_recording() -> Scenario {
    Scenario {
        trusted: false,
        ..scenario(vec![
            OPEN_REPORT,
            card(
                "2 打開工作說明",
                Target::Address(On::Work),
                Fate::LeftOut(Why::Origin(UrlOriginGap::TargetSourceUntrusted)),
            ),
        ])
    }
}

fn a_target_on_a_screen_neither_pass_cited() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開筆記",
            Target::Text(NOTES, On::Scrolled),
            Fate::LeftOut(Why::Frame(TargetFrameGap::FrameNotCited)),
        ),
    ])
}

fn a_target_only_one_pass_cited() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        Card {
            cite_a: &[On::Work, On::Scrolled],
            cite_b: WORK,
            ..card(
                "2 打開筆記",
                Target::Text(NOTES, On::Scrolled),
                Fate::LeftOut(Why::Frame(TargetFrameGap::CitedByOnlyOnePass)),
            )
        },
    ])
}

fn a_target_from_a_window_title() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        card(
            "2 打開計畫書",
            Target::Title(PLAN),
            Fate::LeftOut(Why::Frame(TargetFrameGap::FrameNotRecorded)),
        ),
    ])
}

fn evidence_across_the_browser_and_a_terminal() -> Scenario {
    scenario(vec![
        OPEN_WORK_PAGE,
        Card {
            cite_a: &[On::Work, On::Shell],
            cite_b: &[On::Work, On::Shell],
            ..card(
                "2 打開預算表",
                Target::Text(BUDGET, On::Work),
                Fate::LeftOut(Why::Grant(GrantRejection::Apps)),
            )
        },
    ])
}

fn he_said_links_wait_for_his_press() -> Scenario {
    Scenario {
        url_open: Some(UrlOpenAnswer::OnlyOnMyPress),
        ..scenario(vec![
            OPEN_REPORT,
            card(
                "2 打開工作說明",
                Target::Address(On::Work),
                Fate::LeftOut(Why::Origin(UrlOriginGap::YouSaidPressItYourself)),
            ),
        ])
    }
}

fn he_never_answered_the_link_question() -> Scenario {
    Scenario {
        url_open: None,
        ..scenario(vec![
            OPEN_REPORT,
            card(
                "2 打開工作說明",
                Target::Address(On::Work),
                Fate::LeftOut(Why::Origin(UrlOriginGap::NotAskedYet)),
            ),
        ])
    }
}

/// 第一步之後才出事的那幾組：第二步是一張本來做得到的卡。
fn second_step(fate: Fate) -> Card {
    card("2 打開報告", Target::Text(REPORT, On::Work), fate)
}

fn hands_pulled_after_the_first_step() -> Scenario {
    Scenario {
        after_first_step: Some(After::PullsHands),
        ..scenario(vec![
            OPEN_WORK_PAGE,
            second_step(Fate::NotReached(AbortActor::HandsPulled)),
        ])
    }
}

fn stop_all_after_the_first_step() -> Scenario {
    Scenario {
        after_first_step: Some(After::StopsAll),
        ..scenario(vec![
            OPEN_WORK_PAGE,
            second_step(Fate::NotReached(AbortActor::MasterStopped)),
        ])
    }
}

fn the_target_record_is_gone_by_its_turn() -> Scenario {
    Scenario {
        after_first_step: Some(After::Changes(|dir| {
            let deleted = sql(dir)
                .execute("DELETE FROM facts WHERE raw = ?1", [REPORT])
                .unwrap();
            assert_eq!(deleted, 1, "夾具：刪掉的是那一筆報告");
        })),
        ..scenario(vec![
            OPEN_WORK_PAGE,
            second_step(Fate::Refused(Why::Frame(TargetFrameGap::Forgotten))),
        ])
    }
}

fn the_card_is_pointed_elsewhere_after_yes() -> Scenario {
    Scenario {
        after_first_step: Some(After::Changes(|dir| {
            let conn = sql(dir);
            let fact: i64 = conn
                .query_row(
                    "SELECT id FROM facts WHERE raw = ?1 AND source_kind = 'ocr'",
                    [VISITED_ELSEWHERE],
                    |row| row.get(0),
                )
                .unwrap();
            let changed = conn
                .execute(
                    "UPDATE commitments SET allowed_next_step = ?1, allowed_next_step_fact = ?2
                      WHERE text = '2 打開報告'",
                    rusqlite::params![open_url(VISITED_ELSEWHERE), fact],
                )
                .unwrap();
            assert_eq!(changed, 1, "夾具：改的是第二張卡");
        })),
        ..scenario(vec![
            OPEN_WORK_PAGE,
            second_step(Fate::Refused(Why::Grant(GrantRejection::Target))),
        ])
    }
}

fn the_card_is_dropped_after_yes() -> Scenario {
    Scenario {
        after_first_step: Some(After::Changes(|dir| {
            let mut db = Db::open(&Config::db_path(dir)).unwrap();
            let id = db
                .live_commitments()
                .unwrap()
                .into_iter()
                .find(|card| card.text == "2 打開報告")
                .expect("第二張卡在承諾表上")
                .id;
            sister_core::reviewer::kill_commitment(&mut db, id, "他說不用了", now()).unwrap();
        })),
        ..scenario(vec![
            OPEN_WORK_PAGE,
            second_step(Fate::NotReached(AbortActor::PlanChanged)),
        ])
    }
}

fn the_screen_after_the_first_step_is_another_page() -> Scenario {
    Scenario {
        after_first_step: Some(After::ShowsAnotherPage),
        ..scenario(vec![
            OPEN_WORK_PAGE,
            second_step(Fate::NotReached(AbortActor::UnverifiedStep)),
        ])
    }
}

/// 一個名字一組；表和測試由同一份清單生出來，不會一邊多一組。
macro_rules! scenarios {
    ($($test:ident: $build:ident,)*) => {
        const SCENARIOS: &[(&str, fn() -> Scenario)] = &[$((stringify!($test), $build)),*];
        $(
            #[test]
            fn $test() {
                play(stringify!($test), $build());
            }
        )*
    };
}

scenarios! {
    a_script_on_screen_is_not_offered: a_script_on_screen,
    a_network_share_on_screen_is_not_offered: a_network_share_on_screen,
    a_link_carrying_an_account_is_not_offered: a_link_carrying_an_account,
    a_payment_page_she_never_opened_is_not_offered: a_payment_page_she_never_opened,
    another_page_on_a_site_she_opened_is_not_offered: another_page_on_a_site_she_opened,
    the_page_she_opened_with_another_query_is_not_offered: the_page_she_opened_with_another_query,
    a_link_only_in_the_page_text_is_not_offered: a_link_only_in_the_page_text,
    an_address_from_an_untrusted_recording_is_not_offered: an_address_from_an_untrusted_recording,
    a_target_on_a_screen_neither_pass_cited_is_not_offered: a_target_on_a_screen_neither_pass_cited,
    a_target_only_one_pass_cited_is_not_offered: a_target_only_one_pass_cited,
    a_target_from_a_window_title_is_not_offered: a_target_from_a_window_title,
    evidence_across_the_browser_and_a_terminal_is_not_offered: evidence_across_the_browser_and_a_terminal,
    links_wait_for_his_press_when_he_said_so: he_said_links_wait_for_his_press,
    links_wait_until_he_answers_the_link_question: he_never_answered_the_link_question,
    pulling_the_hands_after_the_first_step_stops_the_second: hands_pulled_after_the_first_step,
    stop_all_after_the_first_step_stops_the_second: stop_all_after_the_first_step,
    a_target_record_gone_by_its_turn_is_refused: the_target_record_is_gone_by_its_turn,
    a_card_pointed_elsewhere_after_yes_is_refused: the_card_is_pointed_elsewhere_after_yes,
    a_card_dropped_after_yes_stops_the_run: the_card_is_dropped_after_yes,
    another_page_after_the_first_step_stops_the_second: the_screen_after_the_first_step_is_another_page,
}

/// 20 組，而且是 20 種不同的停法：每一組被擋下的那一步，停在不同的一道閘門或
/// 同一道閘門的不同一格。表裡兩組停法一樣，就只是同一條測試跑兩次。
#[test]
fn twenty_scenarios_each_stop_a_step_in_a_different_way() {
    assert_eq!(SCENARIOS.len(), 20);
    let mut ways = BTreeSet::new();
    for (name, build) in SCENARIOS {
        let scenario = build();
        let stopped: Vec<Fate> = scenario
            .cards
            .iter()
            .map(|card| card.fate)
            .filter(|fate| *fate != Fate::Done)
            .collect();
        assert_eq!(stopped.len(), 1, "{name}：每一組剛好擋一步");
        assert!(
            scenario.cards.iter().any(|card| card.fate == Fate::Done),
            "{name}：每一組都要有一步真的交到手上，證明這一趟整條路是通的"
        );
        assert!(
            ways.insert(format!("{:?}", stopped[0])),
            "{name}：停法和前面某一組一樣：{:?}",
            stopped[0]
        );
    }
    assert_eq!(ways.len(), 20);
}

// ── 桌面 ────────────────────────────────────────────────────────────────────

fn now() -> i64 {
    sister_core::now_ms()
}

fn sql(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(Config::db_path(dir)).unwrap()
}

/// 錄三張畫面：工作頁面、同一頁往下捲、下一段的 PowerShell。
fn record(dir: &Path) {
    let title = format!("工作頁面 — {PLAN}");
    let work_text = [
        TASK,
        REPORT,
        SCRIPT,
        SHARE,
        BUDGET,
        WITH_ACCOUNT,
        NEVER_VISITED,
        OTHER_PATH,
        OTHER_QUERY,
        VISITED_ELSEWHERE,
    ];
    let steps = serde_json::json!([
        {
            "at_ms": 0,
            "app": "chrome.exe",
            "app_name": "Google Chrome",
            "title": title,
            "url": WORK_PAGE,
            "text": work_text,
        },
        {
            "at_ms": 1000,
            "app": "chrome.exe",
            "app_name": "Google Chrome",
            "title": title,
            "url": WORK_PAGE,
            "text": ["往下捲的那一段", NOTES],
        },
        {
            "at_ms": 2000,
            "app": "powershell.exe",
            "app_name": "Windows PowerShell",
            "title": "Windows PowerShell",
            "text": ["PS> Get-ChildItem"],
        },
        { "at_ms": 3000, "no_screen": true }
    ]);
    let path = dir.join("scenario.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": "takeover-scenario",
            "privacy_context": "clear",
            "system_state": "active",
            "steps": steps,
        }))
        .unwrap(),
    )
    .unwrap();
    crate::ops::replay::run(dir, Config::default(), &path, 1000, false, 0.0, None).unwrap();
}

/// 三張畫面的 id，照時間。
fn frames(dir: &Path) -> BTreeMap<&'static str, i64> {
    let conn = sql(dir);
    let mut stmt = conn
        .prepare("SELECT id, app_id FROM frames ORDER BY ts, id")
        .unwrap();
    let rows: Vec<(i64, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let apps: Vec<Option<&str>> = rows.iter().map(|(_, app)| app.as_deref()).collect();
    assert_eq!(
        apps,
        [
            Some("chrome.exe"),
            Some("chrome.exe"),
            Some("powershell.exe")
        ],
        "夾具：三張畫面都錄進去了"
    );
    [
        ("work", rows[0].0),
        ("scrolled", rows[1].0),
        ("shell", rows[2].0),
    ]
    .into_iter()
    .collect()
}

fn frame_of(frames: &BTreeMap<&'static str, i64>, on: On) -> i64 {
    frames[match on {
        On::Work => "work",
        On::Scrolled => "scrolled",
        On::Shell => "shell",
    }]
}

fn fact_id(dir: &Path, frames: &BTreeMap<&'static str, i64>, target: Target) -> i64 {
    let conn = sql(dir);
    let (query, raw, frame): (&str, &str, Option<i64>) = match target {
        Target::Text(raw, on) => (
            "SELECT id FROM facts WHERE raw = ?1 AND source_kind = 'ocr' AND frame_id = ?2",
            raw,
            Some(frame_of(frames, on)),
        ),
        Target::Address(on) => (
            "SELECT id FROM facts WHERE raw = ?1 AND source_kind = 'url' AND frame_id = ?2",
            WORK_PAGE,
            Some(frame_of(frames, on)),
        ),
        Target::Title(raw) => (
            "SELECT id FROM facts WHERE raw = ?1 AND source_kind = 'window_title'
                AND frame_id IS NULL AND (?2 IS NULL)",
            raw,
            None,
        ),
    };
    let ids: Vec<i64> = conn
        .prepare(query)
        .unwrap()
        .query_map(rusqlite::params![raw, frame], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let [id] = ids[..] else {
        panic!("夾具：{target:?} 要剛好對到一筆 fact，實際 {ids:?}");
    };
    id
}

/// 解釋層那張卡（引用三張畫面），以及一段可信錄製：她打開過工作頁面和另一頁。
fn seed(dir: &Path, frames: &BTreeMap<&'static str, i64>, trusted: bool) {
    let mut db = Db::open(&Config::db_path(dir)).unwrap();
    let work = frame_of(frames, On::Work);
    let at: i64 = sql(dir)
        .query_row("SELECT ts FROM frames WHERE id = ?1", [work], |row| {
            row.get(0)
        })
        .unwrap();
    let segments = db.chapters_for_range(at, at + 4_000).unwrap();
    let segment = segments
        .iter()
        .find(|segment| segment.core_started_at <= at && at < segment.core_ended_at)
        .expect("夾具：有一段蓋住工作頁面");
    let scrolled_at: i64 = sql(dir)
        .query_row(
            "SELECT ts FROM frames WHERE id = ?1",
            [frame_of(frames, On::Scrolled)],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        scrolled_at < segment.core_ended_at,
        "夾具：往下捲的那一屏和第一屏在同一段，上面的 fact 才會列給大腦"
    );
    let evidence: Vec<String> = [On::Work, On::Scrolled, On::Shell]
        .into_iter()
        .map(|on| format!("frame:{}", frame_of(frames, on)))
        .collect();
    db.insert_l2_card(&L2Insert {
        segment_core_start: segment.core_started_at,
        segment_ref: &format!("segment:{}", segment.core_started_at),
        activity: "照工作說明做事",
        entities_json: "[]".into(),
        continues_json: None,
        commitments_json: serde_json::json!([{"text": TASK, "source": TASK, "due_hint": null}])
            .to_string(),
        model_confidence: 0.9,
        evidence_json: serde_json::to_string(&evidence).unwrap(),
        open_questions_json: "[]".into(),
        author: L2Author::Interpreter,
    })
    .unwrap();

    let session = db
        .start_session(sister_core::db::TRUSTED_URL_ORIGIN_PLATFORM, "test")
        .unwrap();
    // 1970 年的紀錄：不落在任何一段裡，不會變成列給大腦的 fact。
    for (index, url) in [WORK_PAGE, VISITED_ELSEWHERE].into_iter().enumerate() {
        db.insert_focus(
            session,
            &FocusEvent {
                ts: 10_000_000 + index as i64,
                kind: FocusKind::UrlChange,
                snapshot: FocusSnapshot {
                    app_id: Some("chrome.exe".into()),
                    app_name: Some("Google Chrome".into()),
                    window_title: Some("她真的打開過的頁面".into()),
                    url: Some(url.into()),
                    pid: Some(1),
                },
            },
        )
        .unwrap();
    }
    if trusted {
        // Replay 永遠拿不到來源票；工作頁面那兩張改成可信錄製寫的。
        let conn = sql(dir);
        for frame in [On::Work, On::Scrolled].map(|on| frame_of(frames, on)) {
            for (table, column) in [
                ("frames", "id"),
                ("text_chunks", "frame_id"),
                ("facts", "frame_id"),
            ] {
                conn.execute(
                    &format!("UPDATE {table} SET session_id = ?1 WHERE {column} = ?2"),
                    rusqlite::params![session, frame],
                )
                .unwrap();
            }
        }
    }
}

/// 假的大腦：兩個 pass 各回一份，照 prompt 裡的 PASS_B 分辨。
fn brain(dir: &Path, cards: &[Card], frames: &BTreeMap<&'static str, i64>) -> Config {
    let answer = |cite: fn(&Card) -> &'static [On]| {
        let commitments: Vec<serde_json::Value> = cards
            .iter()
            .map(|card| {
                let refs: Vec<String> = cite(card)
                    .iter()
                    .map(|on| format!("frame:{}", frame_of(frames, *on)))
                    .collect();
                serde_json::json!({
                    "text": card.text,
                    "stands": true,
                    "kind": "followup",
                    "due_hint": null,
                    "due_source": "explicit",
                    "people": [],
                    "confidence": 0.9,
                    "evidence_refs": refs,
                    "allowed_next_step": {"fact": fact_id(dir, frames, card.target)},
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({ "commitments": commitments })).unwrap()
    };
    let pass_a = dir.join("pass-a.json");
    let pass_b = dir.join("pass-b.json");
    std::fs::write(&pass_a, answer(|card| card.cite_a)).unwrap();
    std::fs::write(&pass_b, answer(|card| card.cite_b)).unwrap();
    let script = dir.join("fake-brain.py");
    std::fs::write(
        &script,
        "import sys\n\
         payload = sys.stdin.buffer.read()\n\
         answer = sys.argv[2] if b'PASS_B' in payload else sys.argv[1]\n\
         sys.stdout.buffer.write(open(answer, 'rb').read())\n",
    )
    .unwrap();
    let args: Vec<String> = [&script, &pass_a, &pass_b]
        .iter()
        .map(|path| serde_json::to_string(&path.to_string_lossy()).unwrap())
        .collect();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[brain]\ncommand = \"python3\"\nargs = [{}]\nreviewer_daily_budget = 40\n",
            args.join(", ")
        ),
    )
    .unwrap();
    Config::load(&config).unwrap()
}

/// 真的手換成這一隻：動作照收，做完在資料庫寫一張「之後」的畫面。
struct Recorded {
    dir: PathBuf,
    db: Db,
    session: i64,
    calls: Vec<ActionSnapshot>,
    after_first_step: Option<After>,
}

impl Recorded {
    fn new(dir: &Path, after_first_step: Option<After>) -> Self {
        let mut db = Db::open(&Config::db_path(dir)).unwrap();
        let session = db.start_session("test/hands", "test").unwrap();
        Self {
            dir: dir.to_path_buf(),
            db,
            session,
            calls: Vec::new(),
            after_first_step,
        }
    }

    fn show(&mut self, action: &ActionSnapshot, twist: Option<After>) {
        let (title, url) = match (action, twist) {
            (_, Some(After::ShowsAnotherPage)) => ("登入".to_string(), Some(ANOTHER_PAGE)),
            (ActionSnapshot::OpenUrl { url }, _) => ("工作頁面".to_string(), Some(url.as_str())),
            (ActionSnapshot::OpenFile { path }, _) => {
                let name = path.to_string_lossy();
                let name = name.rsplit(['\\', '/']).next().unwrap_or_default();
                (format!("{name} - 閱讀器"), None)
            }
            (ActionSnapshot::FocusWindow { title }, _) => (title.clone(), None),
        };
        self.db
            .insert_frame(
                self.session,
                &FrameCapture {
                    ts: now() + 1_000,
                    monitor: 0,
                    width: 1920,
                    height: 1080,
                    dhash: self.calls.len() as u64,
                    image: None,
                    image_ext: "webp",
                    ocr: Vec::new(),
                    assistive: Vec::new(),
                    focus: FocusSnapshot {
                        app_id: Some("chrome.exe".into()),
                        app_name: Some("Google Chrome".into()),
                        window_title: Some(title),
                        url: url.map(str::to_owned),
                        pid: Some(1),
                    },
                },
                None,
                0,
            )
            .unwrap();
    }
}

impl sister_hands::Executor for Recorded {
    fn execute(&mut self, suggestion: &Suggestion) -> std::result::Result<String, ExecutorError> {
        let action = suggestion.snapshot();
        self.calls.push(action.clone());
        let twist = if self.calls.len() == 1 {
            self.after_first_step.take()
        } else {
            None
        };
        self.show(&action, twist);
        match twist {
            Some(After::PullsHands) => {
                sister_hands::kill_switch::pull(&self.dir, now()).unwrap();
            }
            Some(After::StopsAll) => sister_hands::master_stop::engage(&self.dir, now()).unwrap(),
            Some(After::Changes(change)) => change(&self.dir),
            Some(After::ShowsAnotherPage) | None => {}
        }
        Ok(format!("假的手：{}", action.describe()))
    }

    fn hands_attached(&self) -> Attached {
        if sister_hands::master_stop::is_stopped(&self.dir) {
            Attached::MasterStopped {
                since_ms: sister_hands::master_stop::stopped_since(&self.dir),
            }
        } else if sister_hands::kill_switch::is_pulled(&self.dir) {
            Attached::No {
                since_ms: sister_hands::kill_switch::pulled_since(&self.dir),
            }
        } else {
            Attached::Yes
        }
    }
}

/// 錄、整理、提議、他說好、做；然後逐步對帳。
fn play(name: &str, scenario: Scenario) {
    let dir = Tmp::new(&format!("takeover-{name}"));
    record(&dir.0);
    let frames = frames(&dir.0);
    seed(&dir.0, &frames, scenario.trusted);
    let config = brain(&dir.0, &scenario.cards, &frames);
    crate::ops::consent::run(&dir.0, &config, &["cloud-reading".into()], &[], false).unwrap();
    crate::ops::review::run(&dir.0, &config, false, "2h", false, true).unwrap();

    let db = Db::open(&Config::db_path(&dir.0)).unwrap();
    let live = db.live_commitments().unwrap();
    let ids: Vec<i64> = scenario
        .cards
        .iter()
        .map(|card| {
            let found: Vec<&CommitmentRow> =
                live.iter().filter(|row| row.text == card.text).collect();
            let [row] = found[..] else {
                panic!(
                    "{name}：整理完承諾表上要剛好有一張「{}」，實際 {:?}",
                    card.text,
                    live.iter()
                        .map(|row| (&row.text, &row.allowed_next_step))
                        .collect::<Vec<_>>()
                );
            };
            assert!(
                row.allowed_next_step.is_some(),
                "{name}：「{}」要帶著下一步，這一組才測得到接手：{row:?}",
                card.text
            );
            row.id
        })
        .collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "{name}：卡的先後就是表上的先後"
    );
    // 每一步原本要做的事，在任何改動之前讀下來。
    let planned: Vec<ActionSnapshot> = ids
        .iter()
        .map(|id| {
            let row = live.iter().find(|row| row.id == *id).unwrap();
            match sister_hands::commitment_action::parse_allowed_next_step(
                row.allowed_next_step.as_deref(),
            ) {
                sister_hands::commitment_action::AllowedNextStep::Suggestion(button) => {
                    button.snapshot()
                }
                other => panic!("{name}：#{id} 的下一步讀不懂：{other:?}"),
            }
        })
        .collect();

    sister_core::heartbeat::beat(&dir.0, now()).unwrap();
    let mut hands = Recorded::new(&dir.0, scenario.after_first_step);
    let mut out = Vec::new();
    takeover::run_with_output(
        &dir.0,
        &Options {
            status: false,
            url_open: scenario.url_open,
            url_policy_config: None,
        },
        &db,
        &mut Typing::keys("好\n"),
        &mut hands,
        &mut now,
        &mut |_| {},
        &mut out,
    )
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    let events = events(&dir.0);
    let context = format!("{name}\n{out}");

    // 他答了好，那一次答應是有效的。
    assert!(
        matches!(answer(&events), HandoffAnswer::Accepted { .. }),
        "{context}"
    );
    // 手收到的就是對照組那幾步，一步不多、一步不少、照順序。
    let done: Vec<ActionSnapshot> = scenario
        .cards
        .iter()
        .zip(&planned)
        .filter(|(card, _)| card.fate == Fate::Done)
        .map(|(_, action)| action.clone())
        .collect();
    assert_eq!(hands.calls, done, "{context}");

    let offered: BTreeSet<i64> = events
        .iter()
        .find_map(|event| match event {
            ActionEvent::HandoffOffered { grant, .. } => Some(
                grant
                    .approved_commitments()
                    .map(|approved| approved.id)
                    .collect(),
            ),
            _ => None,
        })
        .expect("紀錄上有提議那一列");
    // 輪到過幾步：每一步都先寫一列 `proposed`。沒輪到的那一步連這一列都不該有——
    // 否則它是「走進去才被擋」，和「整輪先停了」是兩道不同的閘門。
    let proposed = events
        .iter()
        .filter(|event| matches!(event, ActionEvent::Proposed { .. }))
        .count();
    for (index, ((card, id), action)) in scenario.cards.iter().zip(&ids).zip(&planned).enumerate() {
        match card.fate {
            Fate::Done => assert!(offered.contains(id), "{context}"),
            Fate::LeftOut(why) => {
                assert!(!offered.contains(id), "擋下的那一步不在授權書上\n{context}");
                let item = left_out(&events)
                    .iter()
                    .find(|item| item.commitment_id == *id)
                    .unwrap_or_else(|| panic!("提議那一列要寫出 #{id} 為什麼不接\n{context}"));
                assert_eq!(item.action.as_ref(), Some(action), "{context}");
                let LeftOutWhy::WouldBeRefused { reason } = &item.why else {
                    panic!("{:?}\n{context}", item.why);
                };
                assert!(
                    why.matches(reason),
                    "要的是 {why:?}，紀錄是 {reason:?}\n{context}"
                );
                assert!(
                    out.contains(&format!("#{id}「{}」", card.text)),
                    "畫面上也要看得到她不接哪一件\n{context}"
                );
            }
            Fate::Refused(why) => {
                assert!(offered.contains(id), "{context}");
                assert_eq!(proposed, index + 1, "輪到了它才被擋\n{context}");
                let refused = refusals(&events);
                let [(_, reason)] = refused[..] else {
                    panic!("剛好一列 refused，實際 {refused:?}\n{context}");
                };
                assert!(
                    why.matches(reason),
                    "要的是 {why:?}，紀錄是 {reason:?}\n{context}"
                );
                assert!(out.contains("沒有做，也沒有交給作業系統："), "{context}");
            }
            Fate::NotReached(by) => {
                assert!(offered.contains(id), "{context}");
                assert_eq!(proposed, index, "沒有輪到它\n{context}");
                assert_eq!(
                    aborted_by(&events),
                    Some((u32::try_from(done.len()).unwrap(), by)),
                    "{context}"
                );
            }
        }
    }
    // 收尾了：鎖放掉，重開之後讀得到這一輪怎麼結束的。
    assert_unlocked(&dir.0);
    let status = status(&dir.0);
    assert!(
        status.contains("接手收尾") || status.contains("接手停下來了"),
        "{status}\n{context}"
    );
}
