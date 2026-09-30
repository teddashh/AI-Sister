//! `sister takeover` 的測試。
//!
//! **交接迴路**（PHASES 第七階段第二格）：提議 → 回答 → 執行 → 回報，以及拒絕、
//! 中止與重開之後讀得回來的狀態。桌面（承諾表、網址紀錄、每一步之後的畫面）和手
//! 都是假的；走的是產品同一支 `run_with_output`，寫的是真的 `action-log.jsonl`。

use super::act::takeover::{self, Options};
use super::act::{StepSource, TargetFrame};
use crate::ops::tmp::Tmp;
use anyhow::Result;
use sister_core::db::{CommitmentRow, FactOrigin, StepFrameRow, TargetApp};
use sister_hands::handoff::{self, HandoffAnswer, LeftOut, LeftOutWhy, LockAttempt};
use sister_hands::semi_action::{AbortActor, GrantRejection, RunConclusionRecord, grant_path};
use sister_hands::url_policy::{TargetAddressOrigin, UrlOpenAnswer, UrlOrigin};
use sister_hands::{
    ActionEvent, ActionLog, ActionSnapshot, ApprovedBy, Attached, ExecutorError, RefusalReason,
    Suggestion, TargetFrameGap, UrlOriginGap,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

mod scenarios;

const T0: i64 = 1_700_000_000_000;
/// 兩個 pass 都指過的那張畫面。
const FRAME: i64 = 10;
const A: &str = "https://a.example.com/1";
const B: &str = "https://b.example.com/2";
const C: &str = "https://c.example.com/3";

fn open_url(url: &str) -> String {
    serde_json::json!({"action": "open_url", "url": url}).to_string()
}

fn url(url: &str) -> ActionSnapshot {
    ActionSnapshot::OpenUrl { url: url.into() }
}

/// 一張進行中的卡：兩個 pass 都指過第 10 張畫面，下一步的目標是第 `id` 筆 fact。
fn card(id: i64, next: &str) -> CommitmentRow {
    let evidence = serde_json::json!([format!("frame:{FRAME}")]).to_string();
    CommitmentRow {
        id,
        text: format!("承諾 {id}"),
        kind: "promise".into(),
        born_from: 1,
        evidence_json: evidence.clone(),
        agreed_evidence_json: Some(evidence),
        people_json: "[]".into(),
        due_hint: None,
        due_source: None,
        due_at: None,
        status: "open".into(),
        confidence: 0.8,
        allowed_next_step: Some(next.to_owned()),
        allowed_next_step_fact: Some(id),
        last_evidence_seen_at: None,
        kill_note: None,
        created_at: 1,
        updated_at: 1,
        tombstoned_at: None,
    }
}

/// 桌面上會變的東西。手和桌面拿同一份，所以「做完第一步之後有人改了承諾表」
/// 寫得出來。
#[derive(Clone, Default)]
struct World {
    cards: Rc<RefCell<Vec<CommitmentRow>>>,
    /// 沒列的網址＝她的紀錄裡看過這個站。
    origins: Rc<RefCell<BTreeMap<String, UrlOrigin>>>,
    /// 現在畫面上的網址。`None`＝一張畫面都沒有。
    screen: Rc<RefCell<Option<String>>>,
}

impl World {
    fn edit_card(&self, id: i64, edit: impl FnOnce(&mut CommitmentRow)) {
        let mut cards = self.cards.borrow_mut();
        edit(
            cards
                .iter_mut()
                .find(|card| card.id == id)
                .expect("那張卡在桌上"),
        );
    }
}

struct Desk {
    world: World,
    /// fact → 它掛的畫面。沒列的＝第 10 張。
    target_frames: BTreeMap<i64, TargetFrame>,
    /// 畫面 → 那張畫面上的 app。
    apps: BTreeMap<i64, String>,
}

fn desk(cards: Vec<CommitmentRow>) -> Desk {
    Desk {
        world: World {
            cards: Rc::new(RefCell::new(cards)),
            ..World::default()
        },
        target_frames: BTreeMap::new(),
        apps: [(FRAME, "chrome.exe".to_string())].into_iter().collect(),
    }
}

impl StepSource for Desk {
    /// 和 `Db::live_commitments` 同一個順序：新的在前。
    fn live_commitments(&self) -> Result<Vec<CommitmentRow>> {
        let mut cards: Vec<CommitmentRow> = self
            .world
            .cards
            .borrow()
            .iter()
            .filter(|card| card.tombstoned_at.is_none())
            .cloned()
            .collect();
        cards.sort_by_key(|card| std::cmp::Reverse((card.created_at, card.id)));
        Ok(cards)
    }
    fn app_for_evidence(&self, r: &sister_core::brain::EvidenceRef) -> Result<Option<String>> {
        Ok(match r {
            sister_core::brain::EvidenceRef::Frame(id) => self.apps.get(id).cloned(),
            sister_core::brain::EvidenceRef::Fact(_) => None,
        })
    }
    fn app_for_target_fact(&self, _id: i64, _expected_raw: &str) -> Result<TargetApp> {
        Ok(TargetApp::Known {
            app: "chrome.exe".into(),
            origin: FactOrigin::Screen,
        })
    }
    fn frame_for_target_fact(&self, id: i64, _expected_raw: &str) -> Result<TargetFrame> {
        Ok(self
            .target_frames
            .get(&id)
            .cloned()
            .unwrap_or(TargetFrame::Known(FRAME)))
    }
    fn site_in_her_record(&self, url: &str) -> Result<UrlOrigin> {
        Ok(self
            .world
            .origins
            .borrow()
            .get(url)
            .copied()
            .unwrap_or(UrlOrigin::InHerRecord))
    }
    fn target_address_on_source_frame(
        &self,
        _id: i64,
        _expected_raw: &str,
    ) -> Result<TargetAddressOrigin> {
        Ok(TargetAddressOrigin::SameFrameAddress)
    }
    /// 每一步之後的那張畫面：時間就是她問的那一刻，網址是手最後開的那一頁。
    fn step_frame_preferring_after(
        &self,
        at_ms: i64,
        _from_ms: i64,
        _to_ms: i64,
    ) -> Result<Option<StepFrameRow>> {
        Ok(self.world.screen.borrow().clone().map(|url| StepFrameRow {
            id: 900,
            ts: at_ms,
            image_path: Some("after.webp".into()),
            window_title: None,
            url: Some(url),
        }))
    }
}

/// 某一步做完之後，對桌面做的改動。
type Change = Box<dyn FnOnce(&World)>;

/// 第幾次交到手上（從 1 算）的時候出什麼事。
enum Twist {
    /// 那一端失敗了。
    Fails,
    /// 做完之後，有人在別的地方拔了手。
    PullsHands,
    /// 做完之後，有人按了全停。
    StopsAll,
    /// 做完了，畫面卻還停在原本那一頁。
    ScreenStays,
    /// 做完之後，桌面被改了。
    Then(Change),
}

struct Hands {
    data_dir: PathBuf,
    world: World,
    calls: Vec<ActionSnapshot>,
    twists: BTreeMap<usize, Twist>,
}

impl sister_hands::Executor for Hands {
    fn execute(&mut self, suggestion: &Suggestion) -> std::result::Result<String, ExecutorError> {
        let action = suggestion.snapshot();
        self.calls.push(action.clone());
        let twist = self.twists.remove(&self.calls.len());
        if matches!(twist, Some(Twist::Fails)) {
            return Err(ExecutorError::platform("那一端沒有反應"));
        }
        if !matches!(twist, Some(Twist::ScreenStays))
            && let ActionSnapshot::OpenUrl { url } = &action
        {
            *self.world.screen.borrow_mut() = Some(url.clone());
        }
        match twist {
            Some(Twist::PullsHands) => {
                sister_hands::kill_switch::pull(&self.data_dir, T0 + 1).unwrap();
            }
            Some(Twist::StopsAll) => {
                sister_hands::master_stop::engage(&self.data_dir, T0 + 1).unwrap();
            }
            Some(Twist::Then(change)) => change(&self.world),
            Some(Twist::Fails | Twist::ScreenStays) | None => {}
        }
        Ok("假的手做了".into())
    }

    fn hands_attached(&self) -> Attached {
        if sister_hands::master_stop::is_stopped(&self.data_dir) {
            Attached::MasterStopped {
                since_ms: sister_hands::master_stop::stopped_since(&self.data_dir),
            }
        } else if sister_hands::kill_switch::is_pulled(&self.data_dir) {
            Attached::No {
                since_ms: sister_hands::kill_switch::pulled_since(&self.data_dir),
            }
        } else {
            Attached::Yes
        }
    }
}

/// 他在鍵盤上打的字。`meanwhile` 發生在提議端出去之後、他按下 Enter 之前。
struct Typing {
    keys: std::io::Cursor<Vec<u8>>,
    meanwhile: Option<Box<dyn FnOnce()>>,
}

impl Typing {
    fn keys(keys: &str) -> Self {
        Self {
            keys: std::io::Cursor::new(keys.as_bytes().to_vec()),
            meanwhile: None,
        }
    }
    fn meanwhile(keys: &str, change: impl FnOnce() + 'static) -> Self {
        Self {
            meanwhile: Some(Box::new(change)),
            ..Self::keys(keys)
        }
    }
    fn fire(&mut self) {
        if let Some(change) = self.meanwhile.take() {
            change();
        }
    }
}

impl Read for Typing {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.fire();
        self.keys.read(buf)
    }
}

impl BufRead for Typing {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.fire();
        self.keys.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.keys.consume(amount);
    }
}

/// 先照 `script` 報時，報完之後每問一次往前 100 毫秒。第一次（沒有 script 時）是 `T0`。
fn clock(script: &[i64]) -> impl FnMut() -> i64 {
    let mut script: VecDeque<i64> = script.iter().copied().collect();
    let mut last = T0 - 100;
    move || {
        last = script.pop_front().unwrap_or(last + 100);
        last
    }
}

fn opts() -> Options {
    Options {
        status: false,
        url_open: Some(UrlOpenAnswer::WhenYouCanNameTheOrigin),
        url_policy_config: None,
    }
}

/// 她在錄：心跳停在 `T0`，這幾秒裡都算活的。
fn recording(dir: &Path) {
    sister_core::heartbeat::beat(dir, T0).unwrap();
}

struct Took {
    out: String,
    calls: Vec<ActionSnapshot>,
    events: Vec<ActionEvent>,
}

fn take_over_with(
    dir: &Path,
    desk: &Desk,
    typing: &mut Typing,
    twists: BTreeMap<usize, Twist>,
    clock: &mut impl FnMut() -> i64,
) -> Result<Took> {
    let mut hands = Hands {
        data_dir: dir.to_path_buf(),
        world: desk.world.clone(),
        calls: Vec::new(),
        twists,
    };
    let mut out = Vec::new();
    takeover::run_with_output(
        dir,
        &opts(),
        desk,
        typing,
        &mut hands,
        clock,
        &mut |_| {},
        &mut out,
    )?;
    Ok(Took {
        out: String::from_utf8(out).unwrap(),
        calls: hands.calls,
        events: events(dir),
    })
}

fn take_over(dir: &Path, desk: &Desk, keys: &str) -> Took {
    take_over_with(
        dir,
        desk,
        &mut Typing::keys(keys),
        BTreeMap::new(),
        &mut clock(&[]),
    )
    .unwrap()
}

fn twisted(dir: &Path, desk: &Desk, twists: impl IntoIterator<Item = (usize, Twist)>) -> Took {
    take_over_with(
        dir,
        desk,
        &mut Typing::keys("好\n"),
        twists.into_iter().collect(),
        &mut clock(&[]),
    )
    .unwrap()
}

fn events(dir: &Path) -> Vec<ActionEvent> {
    let replay = ActionLog::in_data_dir(dir).replay().unwrap();
    assert!(replay.unreadable.is_empty(), "{:?}", replay.unreadable);
    replay.events
}

fn status(dir: &Path) -> String {
    let mut out = Vec::new();
    takeover::status_to(dir, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn kind(event: &ActionEvent) -> &'static str {
    match event {
        ActionEvent::Granted { .. } => "granted",
        ActionEvent::Proposed { .. } => "proposed",
        ActionEvent::Approved { .. } => "approved",
        ActionEvent::Executed { .. } => "executed",
        ActionEvent::Refused { .. } => "refused",
        ActionEvent::StepFinished { .. } => "step_finished",
        ActionEvent::Aborted { .. } => "aborted",
        ActionEvent::Concluded { .. } => "concluded",
        ActionEvent::HandoffOffered { .. } => "handoff_offered",
        ActionEvent::HandoffAnswered { .. } => "handoff_answered",
    }
}

fn kinds(events: &[ActionEvent]) -> Vec<&'static str> {
    events.iter().map(kind).collect()
}

fn answer(events: &[ActionEvent]) -> &HandoffAnswer {
    events
        .iter()
        .find_map(|event| match event {
            ActionEvent::HandoffAnswered { answer, .. } => Some(answer),
            _ => None,
        })
        .expect("紀錄上有回答那一列")
}

fn left_out(events: &[ActionEvent]) -> &[LeftOut] {
    events
        .iter()
        .find_map(|event| match event {
            ActionEvent::HandoffOffered { left_out, .. } => Some(left_out.as_slice()),
            _ => None,
        })
        .expect("紀錄上有提議那一列")
}

fn refusals(events: &[ActionEvent]) -> Vec<(&ActionSnapshot, &RefusalReason)> {
    events
        .iter()
        .filter_map(|event| match event {
            ActionEvent::Refused { action, reason, .. } => Some((action, reason)),
            _ => None,
        })
        .collect()
}

fn aborted_by(events: &[ActionEvent]) -> Option<(u32, AbortActor)> {
    match events.last() {
        Some(ActionEvent::Aborted {
            after_completed_steps,
            by,
            ..
        }) => Some((*after_completed_steps, *by)),
        _ => None,
    }
}

/// 收尾之後鎖一定要放掉：下一次接手拿得到，`--status` 不會把它讀成「還在跑」。
///
/// 等一下再判：同一個測試行程裡，別的測試正在 spawn 子行程（情境組的假大腦）。
/// spawn 到 exec 之間，子行程手上有這個行程所有檔案描述子的拷貝——包括剛放掉的
/// 那把鎖——exec 時才關掉。機器忙的時候那個窗口會被撐開。真的漏掉的鎖不會自己
/// 放掉，兩秒後還是紅的。
fn assert_unlocked(dir: &Path) {
    let mut waited_ms = 0;
    while handoff::is_running(dir).unwrap() {
        assert!(waited_ms < 2_000, "收尾之後還有人拿著接手鎖");
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited_ms += 10;
    }
    assert!(matches!(
        handoff::try_lock(dir).unwrap(),
        LockAttempt::Acquired(_)
    ));
}

fn held(dir: &Path) -> handoff::TakeoverLock {
    match handoff::try_lock(dir).unwrap() {
        LockAttempt::Acquired(lock) => lock,
        LockAttempt::HeldByAnother => panic!("測試自己拿不到接手鎖"),
    }
}

#[test]
fn a_yes_runs_the_offered_steps_in_order_and_reports_each_one() {
    let dir = Tmp::new("takeover-yes");
    recording(&dir.0);
    let desk = desk(vec![card(2, &open_url(B)), card(1, &open_url(A))]);
    let took = take_over(&dir.0, &desk, "好\n");

    assert_eq!(took.calls, vec![url(A), url(B)], "舊的卡先做");
    assert_eq!(
        kinds(&took.events),
        [
            "handoff_offered",
            "handoff_answered",
            "granted",
            "proposed",
            "approved",
            "executed",
            "step_finished",
            "proposed",
            "approved",
            "executed",
            "step_finished",
            "concluded"
        ]
    );
    let (
        ActionEvent::HandoffOffered {
            grant: offered,
            left_out,
            ..
        },
        ActionEvent::HandoffAnswered {
            answer: HandoffAnswer::Accepted { run_id },
            ..
        },
        ActionEvent::Granted {
            grant,
            run_id: Some(granted_run),
            ..
        },
    ) = (&took.events[0], &took.events[1], &took.events[2])
    else {
        panic!("開頭三列不對：{:?}", &took.events[..3]);
    };
    assert_eq!(run_id, granted_run, "回答那一列要指得到底下那一輪");
    assert_eq!(offered, grant, "他答應的就是端出去的那一張");
    assert!(left_out.is_empty());
    assert_eq!(
        grant
            .approved_commitments()
            .map(|approved| (approved.id, approved.action.clone()))
            .collect::<Vec<_>>(),
        [(1, url(A)), (2, url(B))]
    );
    assert!(matches!(
        took.events.last(),
        Some(ActionEvent::Concluded {
            conclusion: RunConclusionRecord::Completed {
                asked: Some(2),
                decided_by: Some(ApprovedBy::StandingGrant),
            },
            ..
        })
    ));
    assert!(
        !grant_path(&dir.0).exists(),
        "接手的授權書只活在那個行程裡，不存成 grant.json"
    );
    assert!(took.out.contains(
        "她可以接手這 2 步，照這個順序做：\n  1. #1「承諾 1」——開啟網址：https://a.example.com/1\n  2. #2「承諾 2」——開啟網址：https://b.example.com/2\n"
    ));
    assert!(took.out.contains("第 1／2 步：#1「承諾 1」"));
    assert!(took.out.contains("第 2／2 步：#2「承諾 2」"));
    assert!(took.out.contains("接手走完了。"));
    assert!(
        took.out
            .contains("這一輪：端出去 2 步，做成 2 步，授權擋掉 0 步，執行失敗 0 步。")
    );
    assert_unlocked(&dir.0);
    let status = status(&dir.0);
    assert!(status.contains("接手收尾："), "{status}");
    assert!(status.contains("做成 2／2 步。"), "{status}");
}

#[test]
fn saying_no_is_recorded_and_nothing_is_done() {
    let dir = Tmp::new("takeover-no");
    recording(&dir.0);
    let took = take_over(&dir.0, &desk(vec![card(1, &open_url(A))]), "不要\n");
    assert!(took.calls.is_empty());
    assert_eq!(kinds(&took.events), ["handoff_offered", "handoff_answered"]);
    assert_eq!(answer(&took.events), &HandoffAnswer::Declined);
    assert!(took.out.contains("好，什麼都沒做。"));
    assert_unlocked(&dir.0);
    assert!(status(&dir.0).contains("你說不要；什麼都沒做。"));
}

#[test]
fn input_that_ends_before_an_answer_is_not_a_no() {
    let dir = Tmp::new("takeover-eof");
    recording(&dir.0);
    let took = take_over(&dir.0, &desk(vec![card(1, &open_url(A))]), "");
    assert!(took.calls.is_empty());
    assert_eq!(answer(&took.events), &HandoffAnswer::NoAnswer);
    assert!(took.out.contains("沒有收到回答；什麼都沒做。"));
    assert!(!took.out.contains("好，什麼都沒做。"));
    assert_unlocked(&dir.0);
    let status = status(&dir.0);
    assert!(status.contains("沒有收到回答，輸入就結束了"), "{status}");
    assert!(!status.contains("你說不要"), "{status}");
}

#[test]
fn an_unclear_answer_is_asked_again_not_guessed() {
    let dir = Tmp::new("takeover-unclear");
    recording(&dir.0);
    let took = take_over(&dir.0, &desk(vec![card(1, &open_url(A))]), "嗯\n好\n");
    assert_eq!(took.out.matches("聽不懂；好／不要").count(), 1);
    assert_eq!(took.out.matches("要她接手嗎？好／不要：").count(), 2);
    assert!(matches!(
        answer(&took.events),
        HandoffAnswer::Accepted { .. }
    ));
    assert_eq!(took.calls, vec![url(A)]);
}

#[test]
fn a_yes_after_the_window_does_nothing() {
    let dir = Tmp::new("takeover-too-late");
    recording(&dir.0);
    let late = handoff::OFFER_WINDOW_MS as i64 + 1;
    let took = take_over_with(
        &dir.0,
        &desk(vec![card(1, &open_url(A))]),
        &mut Typing::keys("好\n"),
        BTreeMap::new(),
        &mut clock(&[T0, T0 + late]),
    )
    .unwrap();
    assert!(took.calls.is_empty());
    assert_eq!(
        answer(&took.events),
        &HandoffAnswer::TooLate {
            answered_after_ms: late as u64
        }
    );
    assert_eq!(kinds(&took.events), ["handoff_offered", "handoff_answered"]);
    assert!(
        took.out
            .contains("的期限；她看過的畫面可能已經不是現在的樣子，所以什麼都沒做。")
    );
    assert_unlocked(&dir.0);
    assert!(status(&dir.0).contains("的期限；什麼都沒做。"));
}

#[test]
fn a_clock_that_went_back_is_its_own_answer() {
    let dir = Tmp::new("takeover-clock-back");
    recording(&dir.0);
    let took = take_over_with(
        &dir.0,
        &desk(vec![card(1, &open_url(A))]),
        &mut Typing::keys("好\n"),
        BTreeMap::new(),
        &mut clock(&[T0, T0 - 1]),
    )
    .unwrap();
    assert!(took.calls.is_empty());
    assert_eq!(answer(&took.events), &HandoffAnswer::ClockWentBack);
    assert!(took.out.contains("系統時鐘比提議端出來的那一刻還早"));
    assert!(status(&dir.0).contains("系統時鐘比提議端出來的那一刻還早"));
}

#[test]
fn pulling_the_hands_after_a_step_stops_the_rest() {
    let dir = Tmp::new("takeover-pulled");
    recording(&dir.0);
    let desk = desk(vec![
        card(1, &open_url(A)),
        card(2, &open_url(B)),
        card(3, &open_url(C)),
    ]);
    let took = twisted(&dir.0, &desk, [(1, Twist::PullsHands)]);
    assert_eq!(took.calls, vec![url(A)]);
    assert_eq!(aborted_by(&took.events), Some((1, AbortActor::HandsPulled)));
    assert!(took.out.contains("手被拔掉，所以這一輪到此為止。"));
    assert!(took.out.contains("沒有輪到 2 步"));
    assert_unlocked(&dir.0);
    let status = status(&dir.0);
    assert!(status.contains("接手停下來了："), "{status}");
    assert!(status.contains("做成 1／3 步。"), "{status}");
}

#[test]
fn a_master_stop_after_a_step_stops_the_rest() {
    let dir = Tmp::new("takeover-master-stop");
    recording(&dir.0);
    let desk = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    let took = twisted(&dir.0, &desk, [(1, Twist::StopsAll)]);
    assert_eq!(took.calls, vec![url(A)]);
    assert_eq!(
        aborted_by(&took.events),
        Some((1, AbortActor::MasterStopped))
    );
    assert!(took.out.contains("全停閘門已生效，所以這一輪到此為止。"));
    assert_unlocked(&dir.0);
}

#[test]
fn a_step_that_failed_stops_the_rest() {
    let dir = Tmp::new("takeover-failed");
    recording(&dir.0);
    let desk = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    let took = twisted(&dir.0, &desk, [(1, Twist::Fails)]);
    assert_eq!(took.calls, vec![url(A)]);
    assert_eq!(aborted_by(&took.events), Some((0, AbortActor::StepNotDone)));
    assert!(took.out.contains("交出去了，那一端失敗了"));
    assert!(took.out.contains("這一步沒有做成，後面的步驟不做。"));
    assert!(!took.events.iter().any(|event| matches!(
        event,
        ActionEvent::Proposed { action, .. } if *action == url(B)
    )));
}

#[test]
fn a_screen_that_does_not_show_the_step_stops_the_rest() {
    let dir = Tmp::new("takeover-unverified");
    recording(&dir.0);
    let desk = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    *desk.world.screen.borrow_mut() = Some("https://old.example.org/".into());
    let took = twisted(&dir.0, &desk, [(1, Twist::ScreenStays)]);
    assert_eq!(took.calls, vec![url(A)]);
    assert_eq!(
        aborted_by(&took.events),
        Some((1, AbortActor::UnverifiedStep))
    );
    assert!(
        took.out
            .contains("畫面沒有對上這一步要開的東西，後面的步驟不做。")
    );
}

#[test]
fn a_card_that_left_the_table_after_yes_stops_the_run_there() {
    let changes: Vec<(&str, Change)> = vec![
        (
            "整理掉",
            Box::new(|world: &World| world.cards.borrow_mut().retain(|card| card.id != 2)),
        ),
        (
            "做完了",
            Box::new(|world: &World| world.edit_card(2, |card| card.status = "done".into())),
        ),
        (
            "下一步被清掉",
            Box::new(|world: &World| world.edit_card(2, |card| card.allowed_next_step = None)),
        ),
    ];
    for (label, change) in changes {
        let dir = Tmp::new("takeover-plan-changed");
        recording(&dir.0);
        let desk = desk(vec![
            card(1, &open_url(A)),
            card(2, &open_url(B)),
            card(3, &open_url(C)),
        ]);
        let took = twisted(&dir.0, &desk, [(1, Twist::Then(change))]);
        assert_eq!(took.calls, vec![url(A)], "{label}");
        assert_eq!(
            aborted_by(&took.events),
            Some((1, AbortActor::PlanChanged)),
            "{label}"
        );
        assert!(
            took.out
                .contains("#2「承諾 2」在你答好之後已經不是進行中、帶著下一步的那張卡了"),
            "{label}"
        );
    }
}

#[test]
fn a_card_rewritten_after_yes_is_refused_by_the_grant_he_signed() {
    // 換成授權書沒有的網址：網址維度先擋。
    // 換成**另一張卡**已經核准的網址：網址維度過得了，承諾維度要擋——
    // 綁卡的五個欄位要落在同一張核准的卡上，不能東拼西湊。
    for (label, rewritten, rejection) in [
        (
            "沒授權的網址",
            "https://evil.example.net/collect",
            GrantRejection::Target,
        ),
        ("卡 1 的網址", A, GrantRejection::Commitment),
    ] {
        let dir = Tmp::new("takeover-rewritten");
        recording(&dir.0);
        let desk = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
        let next = open_url(rewritten);
        let took = twisted(
            &dir.0,
            &desk,
            [(
                1,
                Twist::Then(Box::new(move |world: &World| {
                    world.edit_card(2, |card| card.allowed_next_step = Some(next))
                })),
            )],
        );
        assert_eq!(took.calls, vec![url(A)], "{label}");
        assert_eq!(
            refusals(&took.events),
            [(
                &url(rewritten),
                &RefusalReason::NotCoveredByGrant { rejection }
            )],
            "{label}"
        );
        assert!(took.out.contains("授權擋掉 1 步"), "{label}");
    }
}

#[test]
fn a_card_added_while_he_reads_the_offer_is_not_done() {
    let dir = Tmp::new("takeover-added");
    recording(&dir.0);
    let desk = desk(vec![card(1, &open_url(A))]);
    let cards = desk.world.cards.clone();
    let took = take_over_with(
        &dir.0,
        &desk,
        &mut Typing::meanwhile("好\n", move || {
            cards.borrow_mut().push(card(2, &open_url(B)))
        }),
        BTreeMap::new(),
        &mut clock(&[]),
    )
    .unwrap();
    assert_eq!(took.calls, vec![url(A)]);
    assert!(!took.events.iter().any(|event| matches!(
        event,
        ActionEvent::Proposed { action, .. } if *action == url(B)
    )));
    assert!(took.out.contains("接手走完了。"));
}

/// 提議上說「她不接」的理由，和她真的跑到那一步時寫下的理由，是同一個值。
#[test]
fn a_left_out_reason_is_the_reason_execution_would_write() {
    // 提議的那一刻，b 那個站就不在她的紀錄裡。
    let offered = Tmp::new("takeover-left-out");
    recording(&offered.0);
    let desk_a = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    desk_a
        .world
        .origins
        .borrow_mut()
        .insert(B.into(), UrlOrigin::NotInHerRecord);
    let took = take_over(&offered.0, &desk_a, "好\n");
    assert_eq!(took.calls, vec![url(A)]);
    let [left] = left_out(&took.events) else {
        panic!("應該剛好不接一件：{:?}", left_out(&took.events));
    };
    let LeftOutWhy::WouldBeRefused { reason: said } = &left.why else {
        panic!("{left:?}");
    };

    // 提議的那一刻還在，第一步做完之後才不在了：跑到那一步時被擋。
    let executed = Tmp::new("takeover-refused-later");
    recording(&executed.0);
    let desk_b = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    let took = twisted(
        &executed.0,
        &desk_b,
        [(
            1,
            Twist::Then(Box::new(|world: &World| {
                world
                    .origins
                    .borrow_mut()
                    .insert(B.into(), UrlOrigin::NotInHerRecord);
            })),
        )],
    );
    assert_eq!(took.calls, vec![url(A)]);
    assert_eq!(refusals(&took.events), [(&url(B), said)]);
    assert_eq!(
        said,
        &RefusalReason::UnattendedUrlOriginUnknown {
            why: UrlOriginGap::NotInHerRecord
        }
    );
}

/// 每一種她不接的理由都寫進紀錄、印在提議上，而且那幾步一步都沒有交到手上。
#[test]
fn only_steps_that_pass_every_check_are_offered() {
    let dir = Tmp::new("takeover-offer-filter");
    recording(&dir.0);
    let mut not_cited = card(3, &open_url(C));
    not_cited.allowed_next_step_fact = Some(3);
    let mut two_apps = card(4, &open_url("https://d.example.com/4"));
    two_apps.evidence_json = r#"["frame:11"]"#.into();
    two_apps.agreed_evidence_json = Some(r#"["frame:11"]"#.into());
    let mut done = card(9, &open_url(A));
    done.status = "done".into();
    let mut no_next = card(10, &open_url(A));
    no_next.allowed_next_step = None;
    let mut desk = desk(vec![
        card(1, &open_url(A)),
        card(2, "幫我把款項付掉"),
        not_cited,
        two_apps,
        card(5, &open_url("https://user:pw@e.example.com/5")),
        card(6, &open_url("https://f.example.com/6")),
        card(
            7,
            &serde_json::json!({"action": "open_file", "path": r"C:\work\deploy.ps1"}).to_string(),
        ),
        card(
            8,
            &serde_json::json!({"action": "focus_window", "title": "Windows PowerShell"})
                .to_string(),
        ),
        done,
        no_next,
    ]);
    desk.target_frames.insert(3, TargetFrame::Known(99));
    desk.target_frames.insert(4, TargetFrame::Known(11));
    desk.apps.insert(11, "powershell.exe".into());
    desk.world
        .origins
        .borrow_mut()
        .insert("https://f.example.com/6".into(), UrlOrigin::NotInHerRecord);

    let took = take_over(&dir.0, &desk, "好\n");
    assert_eq!(took.calls, vec![url(A)], "只有第 1 張過得了每一道");
    let reasons: Vec<(i64, LeftOutWhy)> = left_out(&took.events)
        .iter()
        .map(|left| (left.commitment_id, left.why.clone()))
        .collect();
    let refused = |reason| LeftOutWhy::WouldBeRefused { reason };
    assert_eq!(
        reasons,
        [
            (
                2,
                LeftOutWhy::NextStepUnreadable {
                    reason: match sister_hands::commitment_action::parse_allowed_next_step(Some(
                        "幫我把款項付掉"
                    )) {
                        sister_hands::commitment_action::AllowedNextStep::Unparseable {
                            reason,
                            ..
                        } => reason,
                        other => panic!("{other:?}"),
                    }
                }
            ),
            (
                3,
                refused(RefusalReason::UnattendedTargetHasNoCitedFrame {
                    why: TargetFrameGap::FrameNotCited
                })
            ),
            (
                4,
                refused(RefusalReason::NotCoveredByGrant {
                    rejection: GrantRejection::Apps
                })
            ),
            (
                5,
                refused(RefusalReason::NotCoveredByGrant {
                    rejection: GrantRejection::Target
                })
            ),
            (
                6,
                refused(RefusalReason::UnattendedUrlOriginUnknown {
                    why: UrlOriginGap::NotInHerRecord
                })
            ),
            (
                7,
                refused(RefusalReason::TargetRejectedBeforeOs {
                    why: "不會開啟：「.ps1」不在可開啟的檔案類型清單裡".into()
                })
            ),
            (
                8,
                refused(RefusalReason::UnattendedTargetHasNoCitedFrame {
                    why: TargetFrameGap::NoTargetRecorded
                })
            ),
        ]
    );
    assert!(took.out.contains("她可以接手這 1 步"));
    assert!(took.out.contains("她不接的 7 件："));
    for id in 2..=8 {
        assert!(
            took.out.contains(&format!("\n  #{id}「承諾 {id}」")),
            "#{id} 沒有印在提議上：\n{}",
            took.out
        );
    }
    let ActionEvent::HandoffOffered { grant, .. } = &took.events[0] else {
        panic!();
    };
    assert_eq!(
        grant
            .approved_commitments()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [1],
        "授權書的範圍剛好是端出去的那幾步"
    );
}

#[test]
fn at_most_five_steps_are_offered_at_once() {
    let dir = Tmp::new("takeover-five");
    recording(&dir.0);
    let cards = (1..=7)
        .map(|id| card(id, &open_url(&format!("https://s{id}.example.com/"))))
        .collect();
    let took = take_over(&dir.0, &desk(cards), "好\n");
    assert_eq!(takeover::MAX_STEPS, 5);
    assert_eq!(
        took.calls,
        (1..=5)
            .map(|id| url(&format!("https://s{id}.example.com/")))
            .collect::<Vec<_>>()
    );
    assert!(
        took.out
            .contains("另外還有 2 步也過得了這些檢查；一次最多端 5 步，這次先不端。")
    );
}

#[test]
fn with_nothing_to_take_over_there_is_no_offer_and_no_row() {
    let mut done = card(1, &open_url(A));
    done.status = "done".into();
    let mut no_next = card(2, &open_url(A));
    no_next.allowed_next_step = None;
    let mut unreadable = card(3, "幫我寄信");
    unreadable.allowed_next_step = Some("幫我寄信".into());
    for (label, cards, says) in [
        ("空的承諾表", vec![], "承諾表上一張活著的卡都沒有"),
        (
            "都做完了",
            vec![done.clone()],
            "1 張活著的承諾都已經不是進行中",
        ),
        (
            "都沒有下一步",
            vec![done.clone(), no_next.clone()],
            "1 張進行中的承諾都沒有帶著下一步",
        ),
        (
            "都不接",
            vec![no_next, unreadable],
            "2 張進行中的承諾裡，1 張她不接（理由在上面），1 張沒有下一步。",
        ),
    ] {
        let dir = Tmp::new("takeover-nothing");
        recording(&dir.0);
        let took = take_over(&dir.0, &desk(cards), "好\n");
        assert!(took.out.contains(says), "{label}：{}", took.out);
        assert!(!took.out.contains("要她接手嗎"), "{label}");
        assert!(took.events.is_empty(), "{label}：沒有提議就不寫任何一列");
        assert!(took.calls.is_empty(), "{label}");
    }
}

#[test]
fn not_recording_means_no_offer_and_no_row() {
    let dir = Tmp::new("takeover-not-recording");
    let took = take_over(&dir.0, &desk(vec![card(1, &open_url(A))]), "好\n");
    assert!(
        took.out.contains("沒有提議：她從來沒有開始錄。"),
        "{}",
        took.out
    );
    assert!(took.events.is_empty());
    assert!(took.calls.is_empty());

    let stale = Tmp::new("takeover-stale-heartbeat");
    sister_core::heartbeat::beat(&stale.0, T0 - sister_core::heartbeat::STALE_AFTER_MS).unwrap();
    let took = take_over(&stale.0, &desk(vec![card(1, &open_url(A))]), "好\n");
    assert!(took.out.contains("就沒有回報過心跳"), "{}", took.out);
    assert!(took.events.is_empty());
}

#[test]
fn a_stop_switch_means_no_offer_and_no_row() {
    type Stop = fn(&Path);
    let stops: [(&str, Stop); 2] = [
        ("拔手", |dir: &Path| {
            sister_hands::kill_switch::pull(dir, T0 - 1).unwrap();
        }),
        ("全停", |dir: &Path| {
            sister_hands::master_stop::engage(dir, T0 - 1).unwrap();
        }),
    ];
    for (label, stop) in stops {
        let dir = Tmp::new("takeover-stopped");
        recording(&dir.0);
        stop(&dir.0);
        let took = take_over(&dir.0, &desk(vec![card(1, &open_url(A))]), "好\n");
        assert!(
            took.out.contains("沒有提議，也沒有動作會交給作業系統。"),
            "{label}：{}",
            took.out
        );
        assert!(took.events.is_empty(), "{label}");
        assert!(took.calls.is_empty(), "{label}");
    }
}

#[test]
fn a_second_takeover_does_not_queue_behind_the_first() {
    let dir = Tmp::new("takeover-second");
    recording(&dir.0);
    let _first = held(&dir.0);
    let error = take_over_with(
        &dir.0,
        &desk(vec![card(1, &open_url(A))]),
        &mut Typing::keys("好\n"),
        BTreeMap::new(),
        &mut clock(&[]),
    )
    .err()
    .expect("另一個接手拿著鎖的時候不可以提議");
    assert!(
        error
            .to_string()
            .contains("另一個接手還沒收尾，這一趟沒有提議。"),
        "{error}"
    );
    assert!(events(&dir.0).is_empty());
}

#[test]
fn a_pipe_is_refused_before_anything_is_written() {
    let dir = Tmp::new("takeover-pipe");
    let error =
        takeover::run_with_stdin_kind(&dir.0, &opts(), false).expect_err("管子餵進來的「好」不算");
    assert!(error.to_string().contains("stdin 不是終端機"), "{error}");
    assert_eq!(
        std::fs::read_dir(&dir.0).unwrap().count(),
        0,
        "連資料庫、鎖檔和紀錄都沒有碰"
    );
}

/// 行程在任何一列之後被殺掉，檔案裡留下的是完整紀錄的某個前綴。每一個前綴、
/// 鎖在或不在，`--status` 都要講對。
#[test]
fn status_reads_back_every_point_a_run_can_be_killed_at() {
    let ran = Tmp::new("takeover-full-run");
    recording(&ran.0);
    let desk = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    let took = take_over(&ran.0, &desk, "好\n");
    let lines: Vec<String> = std::fs::read_to_string(ActionLog::in_data_dir(&ran.0).path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), took.events.len());
    assert_eq!(lines.len(), 12);
    let ts = crate::fmt::timestamp;

    for kept in 0..=lines.len() {
        let dir = Tmp::new("takeover-prefix");
        if kept > 0 {
            std::fs::write(
                ActionLog::in_data_dir(&dir.0).path(),
                lines[..kept].join("\n") + "\n",
            )
            .unwrap();
        }
        let prefix = &took.events[..kept];
        let done = prefix
            .iter()
            .filter(|event| matches!(event, ActionEvent::StepFinished { .. }))
            .count();
        let (idle, busy) = match kept {
            0 => (
                "還沒有提議過接手。".to_string(),
                "有一個接手正在起來，還沒端出提議。".to_string(),
            ),
            1 => (
                "還沒等到回答那個行程就結束了；什麼都沒做。".to_string(),
                "那個終端機還在等你回答。".to_string(),
            ),
            12 => ("做成 2／2 步。".to_string(), "做成 2／2 步。".to_string()),
            _ => (
                format!(
                    "上一次接手沒有收尾：最後一列在 {}，做完 {done}／2 步；那一輪不會自己接著做。",
                    ts(prefix[kept - 1].at_ms())
                ),
                format!("正在接手：做完 {done}／2 步。"),
            ),
        };
        let got = status(&dir.0);
        assert!(got.contains(&idle), "前 {kept} 列、沒有人拿著鎖：{got}");
        let lock = held(&dir.0);
        let got = status(&dir.0);
        assert!(got.contains(&busy), "前 {kept} 列、有人拿著鎖：{got}");
        drop(lock);
    }

    // `sister forget` 只刪掉提議那一列：還認得出是哪一次，只是不知道端出去幾步。
    let dir = Tmp::new("takeover-offer-forgotten");
    std::fs::write(
        ActionLog::in_data_dir(&dir.0).path(),
        lines[1..].join("\n") + "\n",
    )
    .unwrap();
    let got = status(&dir.0);
    assert!(
        got.contains("做成 2 步（提議那一列不在紀錄裡，不知道端出去幾步）"),
        "{got}"
    );
}

/// 重開之後：上一輪沒有收尾，不會接著做；它做完的那一步也不會再做一次。
#[test]
fn an_interrupted_run_is_not_resumed_and_its_done_step_is_left_out() {
    let ran = Tmp::new("takeover-before-kill");
    recording(&ran.0);
    let first = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    let took = take_over(&ran.0, &first, "好\n");
    let lines: Vec<String> = std::fs::read_to_string(ActionLog::in_data_dir(&ran.0).path())
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let killed_after = took
        .events
        .iter()
        .position(|event| matches!(event, ActionEvent::StepFinished { .. }))
        .unwrap();

    let dir = Tmp::new("takeover-after-kill");
    recording(&dir.0);
    std::fs::write(
        ActionLog::in_data_dir(&dir.0).path(),
        lines[..=killed_after].join("\n") + "\n",
    )
    .unwrap();
    let before = events(&dir.0).len();
    let again = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    let took = take_over(&dir.0, &again, "好\n");

    assert!(
        took.out
            .contains("做完 1 步之後就沒有收尾；那一輪不會接著做。")
    );
    assert_eq!(took.calls, vec![url(B)], "做過的那一步不再做一次");
    let new_rows = &took.events[before..];
    let [left] = left_out(new_rows) else {
        panic!("{:?}", left_out(new_rows));
    };
    assert_eq!(left.commitment_id, 1);
    assert!(matches!(left.why, LeftOutWhy::AlreadyDone { .. }));
    assert!(
        took.out
            .contains("#1「承諾 1」——開啟網址：https://a.example.com/1：這一步")
    );
    assert!(matches!(
        new_rows.last(),
        Some(ActionEvent::Concluded { .. })
    ));
}

/// `sister hands runs`：一次接手在報告上是一組——提議、回答和底下那一輪留在一起。
/// 沒答應的提議自己收成一組：不會被下一次提議吞掉，也不會被讀成「紀錄斷了」。
/// 提議之後就沒有下文的那一組，指路要指到**這一個**資料目錄的 `--status`。
#[test]
fn the_run_report_keeps_each_offer_with_its_answer_and_its_run() {
    let dir = Tmp::new("takeover-report");
    recording(&dir.0);
    let desk = desk(vec![card(1, &open_url(A)), card(2, &open_url(B))]);
    take_over(&dir.0, &desk, "不要\n");
    let took = take_over(&dir.0, &desk, "好\n");
    let (answered_at, run_id) = took
        .events
        .iter()
        .find_map(|event| match event {
            ActionEvent::HandoffAnswered {
                at_ms,
                answer: HandoffAnswer::Accepted { run_id },
                ..
            } => Some((*at_ms, run_id.clone())),
            _ => None,
        })
        .expect("第二次他說好");
    // 第三次：提議端出來了，行程在他回答之前就結束了——紀錄上只剩提議那一列。
    let other = Tmp::new("takeover-report-unanswered");
    recording(&other.0);
    take_over(&other.0, &desk, "不要\n");
    let offered_only = std::fs::read_to_string(ActionLog::in_data_dir(&other.0).path())
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned();
    let path = ActionLog::in_data_dir(&dir.0).path().to_owned();
    let mut log = std::fs::read_to_string(&path).unwrap();
    log.push_str(&offered_only);
    log.push('\n');
    std::fs::write(&path, log).unwrap();

    let mut text = Vec::new();
    super::act::runs_to(&dir.0, 20, false, &mut text).unwrap();
    let text = String::from_utf8(text).unwrap();
    let groups: Vec<&str> = text.split("── 第 ").skip(1).collect();
    let [declined, accepted, unanswered] = groups.as_slice() else {
        panic!("應該是三組：{text}");
    };
    assert!(declined.contains("接手提議："), "{declined}");
    assert!(declined.contains("他說不要；什麼都沒做"), "{declined}");
    assert!(
        !declined.contains("收尾："),
        "說不要就收完了，不是紀錄斷了：{declined}"
    );
    assert!(accepted.contains("接手提議："), "{accepted}");
    assert!(
        accepted.contains(&format!("他說好；這一輪是 {run_id}")),
        "{accepted}"
    );
    assert!(
        accepted.contains("授權書就是上面提議的那一張"),
        "{accepted}"
    );
    assert!(
        accepted.contains(&format!("審計 ID：run={run_id}；")),
        "{accepted}"
    );
    assert!(accepted.contains("第 2 步："), "{accepted}");
    assert!(
        !accepted.contains("這一輪的紀錄到這裡就沒有了"),
        "{accepted}"
    );
    let status_cmd = super::cmd(&dir.0, "takeover --status");
    assert!(status_cmd.contains("--data-dir"), "前提：這不是預設目錄");
    assert!(
        unanswered.contains(&format!(
            "收尾：提議之後沒有回答那一列——可能還在等他回答，也可能那個行程在他回答之前就結束了；`{status_cmd}` 分得出是哪一種。"
        )),
        "{unanswered}"
    );

    let mut json = Vec::new();
    super::act::runs_to(&dir.0, 20, true, &mut json).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let runs = json["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3, "{json:#}");
    let (declined, accepted, unanswered) = (&runs[0], &runs[1], &runs[2]);
    assert_eq!(declined["handoff"]["answer"]["answer"], "declined");
    assert_eq!(declined["complete"], true);
    assert_eq!(declined["run_id"], serde_json::Value::Null);
    assert_eq!(declined["summary"]["steps"], 0);
    assert_eq!(accepted["handoff"]["answer"]["answer"], "accepted");
    assert_eq!(accepted["handoff"]["answer"]["run_id"], run_id.as_str());
    assert_eq!(accepted["run_id"], run_id.as_str());
    assert_eq!(accepted["complete"], true);
    assert_eq!(accepted["summary"]["steps"], 2);
    assert_eq!(accepted["summary"]["succeeded"], 2);
    assert_eq!(accepted["summary"]["approved_by_grant"], 2);
    assert_eq!(accepted["handoff"]["left_out"], 0);
    assert_ne!(
        accepted["handoff"]["offered_at_ms"], answered_at,
        "前提：提議和回答不是同一刻"
    );
    assert_eq!(
        accepted["started_at_ms"], answered_at,
        "開始是拿到授權書那一刻，和文字報告的「開始：」同一個時間"
    );
    assert_eq!(unanswered["complete"], false);
    assert_eq!(unanswered["handoff"]["answer"], serde_json::Value::Null);
    assert_eq!(
        unanswered["handoff"]["answered_at_ms"],
        serde_json::Value::Null
    );
    assert_eq!(unanswered["handoff"]["left_out"], 0);
}
