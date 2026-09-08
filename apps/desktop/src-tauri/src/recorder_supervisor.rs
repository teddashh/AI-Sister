//! `sister-desktop` 自己開起來的 recorder supervisor。
//!
//! 只有這條 worker thread 握 [`Child`]。按鈕、系統匣、Windows 登入 intent、
//! retry timer 與 quit 都排進同一個 channel，才不會兩個入口同時看見「沒有 child」
//! 又各自 spawn 一份。重試政策在 `sister-core::recorder_watchdog`；這裡只負責把
//! 真實 process／heartbeat／consent／stop intent 翻成那個純狀態機的事件。

use serde::Serialize;
use sister_core::recorder_watchdog as policy;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use tauri::Emitter;

const WORKER_TICK: Duration = Duration::from_millis(200);
const OCCUPIED_RECHECK: policy::RetryDelay =
    policy::RetryDelay::from_millis(250).expect("non-zero retry delay");
const COMMAND_REPLY_LIMIT: Duration = Duration::from_secs(10);
const LOGIN_PREFLIGHT_RECHECK: Duration = Duration::from_millis(500);
// 最長的正常 Thinking 上限是兩輪各 120 秒；多留一分鐘給 session teardown、
// 防毒暫壓 lock 與排程延遲，但不能讓一個壞掉的登入 intent 永遠輪詢。
const LOGIN_PREFLIGHT_LIMIT: Duration = Duration::from_secs(5 * 60);
const SUPERVISED_ARGUMENT: &str = "--desktop-supervised";
const CHANGED_EVENT: &str = "recorder-supervisor-changed";
const START_PENDING: u8 = 0;
const START_CANCELLED: u8 = 1;
const START_COMMITTED: u8 = 2;

fn cancel_uncommitted_start(commit: &AtomicU8) -> bool {
    commit
        .compare_exchange(
            START_PENDING,
            START_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn commit_uncancelled_start(commit: &AtomicU8) -> bool {
    commit
        .compare_exchange(
            START_PENDING,
            START_COMMITTED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SupervisorPhase {
    Stopped,
    /// Owned child 已退出，但它最後一顆心跳仍在安全期限內；不能把那顆舊證據
    /// 說成現在仍在錄，也不能在它過期前提供 Start。
    Cooling,
    Starting,
    Running,
    Backoff,
    GaveUp,
    /// 另一個入口開的 recorder；desktop 只觀察，不持有 Child、也不接管重試。
    External,
    /// 已留下停止外部 recorder 的 durable intent，但尚未看到停止墓碑。
    StoppingExternal,
    /// 真人已要求停止，但 durable stop 尚在送達或已送達失敗。這個狀態和一般
    /// ownership uncertainty 分開，讓系統匣只重送 Stop，永遠不把它翻成 Start。
    StopUndelivered,
    Uncertain,
    Quitting,
}

/// Renderer 讀到的是 supervisor 自己能證明的狀態，不拿 heartbeat 的 `none` 猜。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SupervisorView {
    pub(crate) phase: SupervisorPhase,
    pub(crate) failures: u8,
    pub(crate) message: Option<String>,
}

impl SupervisorView {
    pub(crate) fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            phase: SupervisorPhase::Uncertain,
            failures: 0,
            message: Some(reason.into()),
        }
    }

    fn initial() -> Self {
        Self {
            phase: SupervisorPhase::Stopped,
            failures: 0,
            message: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Handle {
    sender: mpsc::Sender<Message>,
    view: Arc<Mutex<SupervisorView>>,
    /// 只在 worker 已留下 durable stop（或能證明沒有 owned child）後變成 true。
    /// 若同步呼叫先 timeout、worker 隨後才完成，下一次 quit 仍能讀到這份證據。
    quit_confirmed: Arc<AtomicBool>,
}

impl Handle {
    pub(crate) fn spawn(app: tauri::AppHandle, data_dir: Option<PathBuf>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let view = Arc::new(Mutex::new(SupervisorView::initial()));
        let worker_view = Arc::clone(&view);
        let quit_confirmed = Arc::new(AtomicBool::new(false));
        let worker_quit_confirmed = Arc::clone(&quit_confirmed);
        std::thread::Builder::new()
            .name("sister-recorder-supervisor".into())
            .spawn(move || {
                Worker::new(app, data_dir, receiver, worker_view, worker_quit_confirmed).run();
            })
            .expect("spawn recorder supervisor");
        Self {
            sender,
            view,
            quit_confirmed,
        }
    }

    pub(crate) fn view(&self) -> SupervisorView {
        self.view.lock().expect("recorder supervisor view").clone()
    }

    /// 人按下按鈕；要等到真正 spawn 成功才回 Ok。
    pub(crate) fn explicit_start(&self) -> Result<(), String> {
        let (sender, receive) = mpsc::sync_channel(1);
        let commit = Arc::new(AtomicU8::new(START_PENDING));
        self.sender
            .send(Message::Start {
                source: StartSource::Explicit,
                reply: Some(StartReply {
                    sender,
                    commit: Arc::clone(&commit),
                }),
            })
            .map_err(|_| "recorder supervisor 已經停止".to_owned())?;
        match receive.recv_timeout(COMMAND_REPLY_LIMIT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // worker 還在 consent/control/heartbeat preflight 時，取消會贏；它
                // 稍後拿到鎖也不准 spawn。若 worker 已取得 commit，這裡就等真
                // 結果，不能先回失敗、稍後卻開始錄。
                if cancel_uncommitted_start(&commit) {
                    Err("十秒內沒有完成啟動前檢查；這次啟動已取消".to_owned())
                } else {
                    receive
                        .recv()
                        .map_err(|_| "recorder supervisor 在已承諾啟動後停止".to_owned())?
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("recorder supervisor 在回覆啟動結果前停止".to_owned())
            }
        }
    }

    /// Windows Run／第二個 login instance 的 intent。結果走持久 view + event，
    /// 不把背景登入卡在一個沒有人等的同步回條上。
    pub(crate) fn login_start(&self) -> Result<(), String> {
        self.sender
            .send(Message::Start {
                source: StartSource::Login,
                reply: None,
            })
            .map_err(|_| "recorder supervisor 已經停止，登入啟動沒有交出去".to_owned())
    }

    pub(crate) fn stop(&self) -> Result<(), String> {
        self.call(|reply| Message::Stop { reply })
    }

    /// LocalRecording 撤回在任何 durable I/O 前先關閉 background Login/watchdog。
    /// 這個 message 自己零 I/O；即使後面的 consent transaction 失敗，automatic
    /// start latch 仍保持關閉，直到真人成功 commit 一次 Explicit Start。
    pub(crate) fn cancel_automatic_for_consent_revoke(&self) -> Result<(), String> {
        self.call(|reply| Message::CancelAutomaticForConsentRevoke { reply })
    }

    pub(crate) fn quit(&self) -> Result<(), String> {
        match self.call(|reply| Message::Quit { reply }) {
            Err(_) if self.quit_confirmed.load(Ordering::Acquire) => Ok(()),
            result => result,
        }
    }

    fn call(
        &self,
        make: impl FnOnce(mpsc::SyncSender<Result<(), String>>) -> Message,
    ) -> Result<(), String> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.sender
            .send(make(reply))
            .map_err(|_| "recorder supervisor 已經停止".to_owned())?;
        receive
            .recv_timeout(COMMAND_REPLY_LIMIT)
            .map_err(|_| "recorder supervisor 在十秒內沒有回話".to_owned())?
    }
}

enum Message {
    Start {
        source: StartSource,
        reply: Option<StartReply>,
    },
    Stop {
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    CancelAutomaticForConsentRevoke {
        reply: mpsc::SyncSender<Result<(), String>>,
    },
    Quit {
        reply: mpsc::SyncSender<Result<(), String>>,
    },
}

struct StartReply {
    sender: mpsc::SyncSender<Result<(), String>>,
    commit: Arc<AtomicU8>,
}

/// 不論成功或失敗都先交回真正結果；只有「成功但呼叫端已經 timeout」才由
/// worker 自己完成 app exit。把 `send` 放在 `&&` 右邊會讓 Err 被 short-circuit
/// 掉，呼叫端反而收到假的「十秒內沒有回話」。
fn reply_to_quit(reply: mpsc::SyncSender<Result<(), String>>, result: Result<(), String>) -> bool {
    let succeeded = result.is_ok();
    let receiver_gone = reply.send(result).is_err();
    succeeded && receiver_gone
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartSource {
    Explicit,
    Login,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginStopPolicy {
    Absent,
    DesktopQuit,
    Preserve(sister_core::control::StopReason),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginRetryDecision {
    Retry,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginOccupancyDecision {
    Start,
    Retry,
    External,
}

fn login_occupancy_decision(
    stop: LoginStopPolicy,
    lease_available: bool,
    presence: sister_core::heartbeat::Presence,
) -> LoginOccupancyDecision {
    match stop {
        LoginStopPolicy::Absent => {
            if lease_available
                && matches!(
                    presence,
                    sister_core::heartbeat::Presence::NeverStarted
                        | sister_core::heartbeat::Presence::Stopped { .. }
                )
            {
                LoginOccupancyDecision::Start
            } else {
                LoginOccupancyDecision::External
            }
        }
        LoginStopPolicy::DesktopQuit => {
            if !lease_available
                || matches!(
                    presence,
                    sister_core::heartbeat::Presence::Live(_)
                        | sister_core::heartbeat::Presence::Thinking { .. }
                )
            {
                LoginOccupancyDecision::Retry
            } else {
                LoginOccupancyDecision::Start
            }
        }
        LoginStopPolicy::Preserve(_) | LoginStopPolicy::Unknown => {
            unreachable!("stop policy is rejected before occupancy")
        }
    }
}

/// Login retry 只屬於全新 worker，不借 watchdog generation/failure budget。
/// `heartbeat_at` 只保留 transient 讀取看過的診斷證據；ownership 決策只用
/// `observed_owner`，絕不把 Login 的 observation 冒充 desktop-owned cutoff。
#[derive(Debug, Clone, Copy)]
struct LoginPreflight {
    heartbeat_at: Option<sister_core::Millis>,
    observed_owner: bool,
    retry_at: Instant,
    expires_at: Instant,
}

fn login_retry_decision(state: policy::State, before_deadline: bool) -> LoginRetryDecision {
    if !matches!(
        state,
        policy::State::Stopped {
            reason: policy::StoppedBy::NeverStarted,
            ..
        }
    ) {
        return LoginRetryDecision::Cancelled;
    }
    if before_deadline {
        LoginRetryDecision::Retry
    } else {
        LoginRetryDecision::Expired
    }
}

enum LoginAttemptError {
    Retry {
        message: String,
        heartbeat_at: Option<sister_core::Millis>,
        observed_owner: bool,
    },
    External(String),
    Expired,
    Cancel(String),
}

fn login_stop_policy(intent: sister_core::control::StopIntent) -> LoginStopPolicy {
    match intent {
        sister_core::control::StopIntent::Absent => LoginStopPolicy::Absent,
        sister_core::control::StopIntent::Pending(
            sister_core::control::StopReason::DesktopQuit,
        )
        | sister_core::control::StopIntent::Consumed(
            sister_core::control::StopReason::DesktopQuit,
        ) => LoginStopPolicy::DesktopQuit,
        sister_core::control::StopIntent::Pending(reason)
        | sister_core::control::StopIntent::Consumed(reason) => LoginStopPolicy::Preserve(reason),
        sister_core::control::StopIntent::Uncheckable => LoginStopPolicy::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoginHeartbeatSnapshotError {
    Changed,
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginChangedSnapshotDecision {
    Retry,
    External,
}

fn login_changed_snapshot_decision(
    stop_policy: Option<LoginStopPolicy>,
) -> LoginChangedSnapshotDecision {
    match stop_policy {
        Some(LoginStopPolicy::Absent) => LoginChangedSnapshotDecision::External,
        Some(LoginStopPolicy::DesktopQuit) | None => LoginChangedSnapshotDecision::Retry,
        Some(LoginStopPolicy::Preserve(_) | LoginStopPolicy::Unknown) => {
            unreachable!("rejected stop policy never reaches heartbeat")
        }
    }
}

fn login_heartbeat_snapshot_with(
    mut read_presence: impl FnMut() -> sister_core::heartbeat::Presence,
    mut read_last_beat: impl FnMut() -> Option<sister_core::Millis>,
) -> Result<
    (
        sister_core::heartbeat::Presence,
        Option<sister_core::Millis>,
    ),
    LoginHeartbeatSnapshotError,
> {
    let presence = read_presence();
    let heartbeat_at = match presence {
        sister_core::heartbeat::Presence::Unreadable => {
            return Err(LoginHeartbeatSnapshotError::Unknown(
                "讀不懂 recording.beat；登入啟動沒有把未知狀態猜成可以等待或啟動".to_owned(),
            ));
        }
        sister_core::heartbeat::Presence::Live(_) => {
            Some(read_last_beat().ok_or(LoginHeartbeatSnapshotError::Changed)?)
        }
        sister_core::heartbeat::Presence::Thinking { at, .. }
        | sister_core::heartbeat::Presence::Stalled { at, .. } => Some(at),
        sister_core::heartbeat::Presence::NeverStarted
        | sister_core::heartbeat::Presence::Stopped { .. } => None,
    };
    Ok((presence, heartbeat_at))
}

fn login_heartbeat_snapshot(
    data_dir: &Path,
) -> Result<
    (
        sister_core::heartbeat::Presence,
        Option<sister_core::Millis>,
    ),
    LoginHeartbeatSnapshotError,
> {
    login_heartbeat_snapshot_with(
        || sister_core::heartbeat::presence(data_dir, sister_core::now_ms()),
        || sister_core::heartbeat::last_beat(data_dir),
    )
}

fn cancel_login_preflight(pending: &mut Option<LoginPreflight>) {
    *pending = None;
}

fn login_commit_before_deadline(pending: Option<LoginPreflight>, now: Instant) -> bool {
    pending.is_some_and(|pending| now < pending.expires_at)
}

fn login_observed_owner_blocks_absent(
    stop_policy: LoginStopPolicy,
    pending: Option<LoginPreflight>,
) -> bool {
    stop_policy == LoginStopPolicy::Absent && pending.is_some_and(|pending| pending.observed_owner)
}

fn login_waiting_view(message: String) -> SupervisorView {
    SupervisorView {
        phase: SupervisorPhase::Uncertain,
        failures: 0,
        message: Some(format!(
            "Windows 登入啟動遇到暫時無法判定的 preflight barrier；尚未清除停止意圖，也尚未啟動 recorder。會在有界期限內完整重試：{message}"
        )),
    }
}

/// Parent 的 shared consent transaction 只跨過不可分割的 clear→spawn barrier。
/// `spawn` 回傳就立刻 drop；不能等 child heartbeat，因為 loader hang／suspend 會
/// 讓 revoke writer 永遠拿不到 exclusive lock。Child 另有自己的 nonblocking
/// guard：writer 先贏就 Busy/zero beat，child 先贏才是排在 revoke 之前的合法 start。
fn with_parent_start_consent<Guard, Result>(
    guard: Guard,
    spawn: impl FnOnce() -> Result,
) -> Result {
    let result = spawn();
    drop(guard);
    result
}

fn take_first_login_intent(seen: &mut bool) -> bool {
    if *seen {
        false
    } else {
        *seen = true;
        true
    }
}

fn retained_login_terminal_message(
    state: policy::State,
    override_present: bool,
    terminal: Option<&str>,
) -> Option<&str> {
    (!override_present
        && matches!(
            state,
            policy::State::Stopped {
                reason: policy::StoppedBy::NeverStarted,
                ..
            } | policy::State::External { .. }
        ))
    .then_some(terminal)
    .flatten()
}

fn automatic_effect_blocked_by_human(cancelled: bool, effect: policy::Effect) -> bool {
    cancelled
        && matches!(
            effect,
            policy::Effect::Spawn { .. } | policy::Effect::ScheduleRetry { .. }
        )
}

fn automatic_login_blocked_by_human(cancelled: bool) -> bool {
    cancelled
}

fn commit_human_explicit_start_if_spawned<T, E>(
    spawn_result: &Result<T, E>,
    cancelled: &mut bool,
    stop_delivery_failure: &mut Option<String>,
) -> bool {
    if spawn_result.is_err() {
        return false;
    }
    *cancelled = false;
    *stop_delivery_failure = None;
    true
}

fn stop_delivery_pending_view(state: policy::State) -> SupervisorView {
    SupervisorView {
        phase: SupervisorPhase::StopUndelivered,
        failures: state.failures().get(),
        message: Some(
            "真人已要求停止，自動啟動已取消；正在把停止要求送到磁碟，但尚未證明 recorder 已停止。"
                .to_owned(),
        ),
    }
}

fn stop_delivery_failure_view(
    state: policy::State,
    has_child: bool,
    error: &str,
) -> SupervisorView {
    let status = if has_child
        || matches!(
            state,
            policy::State::External { .. } | policy::State::ExternalStopping { .. }
        ) {
        "目前 recorder 仍可能在跑"
    } else {
        "目前已觀察不到 owned child；即使先前退出是失敗，也不會自動重開"
    };
    SupervisorView {
        phase: SupervisorPhase::StopUndelivered,
        failures: state.failures().get(),
        message: Some(format!(
            "真人已要求停止，自動啟動已在記憶體中取消，但停止要求沒有送達磁碟（{error}）；{status}。desktop 不會自動重開；修好後可再按一次停止。"
        )),
    }
}

fn consent_revoke_cancellation_view(state: policy::State, has_child: bool) -> SupervisorView {
    let phase = if has_child && matches!(state, policy::State::Starting { .. }) {
        SupervisorPhase::Starting
    } else if has_child && matches!(state, policy::State::Running { .. }) {
        SupervisorPhase::Running
    } else if matches!(state, policy::State::External { .. }) {
        SupervisorPhase::External
    } else if matches!(
        state,
        policy::State::Stopped {
            reason: policy::StoppedBy::NeverStarted,
            ..
        }
    ) {
        SupervisorPhase::Stopped
    } else {
        SupervisorPhase::Uncertain
    };
    SupervisorView {
        phase,
        failures: state.failures().get(),
        message: Some(
            "真人已開始撤回第一張同意書；pending Login 與 watchdog 自動啟動已在記憶體中取消。這句不宣稱同意書或停止意圖已成功寫入磁碟。"
                .to_owned(),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetachedHeartbeat {
    Vacant,
    OldOwned,
    External,
    ExternalUncertain,
    ExternalGone,
    Unreadable,
}

struct RetryStartBarrier {
    consent: sister_core::consent::RecordingStartGuard,
    stop: sister_core::control::StartTransactionGuard,
    lease: sister_core::recorder_lease::RecorderLease,
}

struct Spawned {
    child: Child,
    /// 牆上時間只給既有的 `safe_to_kill_spawn` 與 heartbeat owner 判斷。
    at: sister_core::Millis,
    generation: policy::Generation,
}

struct Worker {
    app: tauri::AppHandle,
    data_dir: Option<PathBuf>,
    receiver: mpsc::Receiver<Message>,
    published: Arc<Mutex<SupervisorView>>,
    policy: policy::State,
    child: Option<Spawned>,
    retry: Option<(policy::Generation, Instant)>,
    /// Windows login 的 pre-spawn transient barrier。不是 recorder crash retry，
    /// 不推進或重設 watchdog failure budget。
    login_preflight: Option<LoginPreflight>,
    /// 同一個 worker 只承接第一份 login intent；cancel／expiry 後的 secondary
    /// instance 不可另開一個新的 automatic window。
    login_intent_seen: bool,
    /// Login 終止原因不能被下一個 detached-heartbeat no-op tick 擦掉。
    login_terminal_message: Option<String>,
    /// 真人 Stop 一送進 worker 就先關閉所有 automatic spawn，不能把是否復活綁在
    /// durable stop write 成不成功。只有之後的真人 Explicit Start 可清掉。
    automatic_start_cancelled_by_human: bool,
    /// `request_stop` 正在執行；這段不能顯示舊 Backoff 倒數。
    stop_delivery_pending: bool,
    /// Stop request 寫失敗時保留真相；不能把 in-memory cancellation 說成 child 已停。
    stop_delivery_failure: Option<String>,
    /// 第一張同意書的撤回動作一到 worker 就取消 automatic effects；這份投影要
    /// 壓掉舊 Backoff 倒數，但不能冒充 durable consent/stop 已成功寫入。
    consent_revoke_cancellation: bool,
    /// 這個時刻之後還繼續更新的 heartbeat 不可能屬於已退出的 owned child。
    /// `None` 代表 desktop 這一生還沒擁有過 child，任何新鮮 heartbeat 都是 external。
    owned_child_observed_gone_at: Option<sister_core::Millis>,
    /// 同一顆五秒 heartbeat 會被 worker 讀約 25 次；只有 `(at, phase)` 換了
    /// 才是健康區間向前走的新證據。
    last_observed_heartbeat: Option<(sister_core::Millis, sister_core::heartbeat::Phase)>,
    last_error: Option<String>,
    /// Durable stop 寫失敗後，即使 child 又退出，也不能把畫面改回假的
    ///「正在結束」。真人重試 quit 成功以前，保留可操作的失敗理由。
    quit_failure: Option<String>,
    quit_confirmed: Arc<AtomicBool>,
    clock_started: Instant,
    done: bool,
}

impl Worker {
    fn new(
        app: tauri::AppHandle,
        data_dir: Option<PathBuf>,
        receiver: mpsc::Receiver<Message>,
        published: Arc<Mutex<SupervisorView>>,
        quit_confirmed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            app,
            data_dir,
            receiver,
            published,
            policy: policy::State::initial(),
            child: None,
            retry: None,
            login_preflight: None,
            login_intent_seen: false,
            login_terminal_message: None,
            automatic_start_cancelled_by_human: false,
            stop_delivery_pending: false,
            stop_delivery_failure: None,
            consent_revoke_cancellation: false,
            owned_child_observed_gone_at: None,
            last_observed_heartbeat: None,
            last_error: None,
            quit_failure: None,
            quit_confirmed,
            clock_started: Instant::now(),
            done: false,
        }
    }

    fn run(mut self) {
        while !self.done {
            match self.receiver.recv_timeout(self.wait_for()) {
                Ok(message) => self.handle(message),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.handle_disconnected();
                    break;
                }
            }
            self.poll_child();
            self.poll_stop_intent();
            self.fire_login_preflight_if_due();
            self.poll_detached_heartbeat();
            self.fire_retry_if_due();
        }
    }

    fn wait_for(&self) -> Duration {
        let now = Instant::now();
        let watchdog = self
            .retry
            .map(|(_, due)| due.saturating_duration_since(now))
            .unwrap_or(WORKER_TICK);
        let login = self
            .login_preflight
            .map(|pending| pending.retry_at.saturating_duration_since(now))
            .unwrap_or(WORKER_TICK);
        watchdog.min(login).min(WORKER_TICK)
    }

    fn now(&self) -> policy::MonotonicMillis {
        policy::MonotonicMillis::new(
            self.clock_started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        )
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Start { source, reply } => {
                if source == StartSource::Login {
                    debug_assert!(reply.is_none(), "login start has no synchronous receiver");
                    self.handle_login_start();
                    return;
                }
                // 真人 Start 取代尚未完成的背景 Login；即使真人這次自己失敗，也不
                // 能讓五分鐘前的 automatic intent 稍後在背後成功。
                cancel_login_preflight(&mut self.login_preflight);
                self.login_terminal_message = None;
                let result = self.start(source, reply.as_ref());
                if let Some(reply) = reply {
                    let _ = reply.sender.send(result);
                } else if let Err(error) = result {
                    tracing::warn!("登入時沒有啟動 recorder：{error}");
                }
            }
            Message::Stop { reply } => {
                // 先取消 automatic intent，再做可能失敗的 durable write。若順序
                // 相反，request_stop 的 I/O error 會留下 NeverStarted + due Login，
                // worker 稍後可能在真人剛按 Stop 後自行開始錄。
                cancel_login_preflight(&mut self.login_preflight);
                self.automatic_start_cancelled_by_human = true;
                self.retry = None;
                self.stop_delivery_pending = true;
                self.login_terminal_message = Some("登入啟動已由人工停止取消。".to_owned());
                self.publish_from_policy(None);
                let result = self.stop(policy::StoppedBy::Requested);
                self.stop_delivery_pending = false;
                if let Err(error) = result.as_ref() {
                    self.stop_delivery_failure = Some(error.clone());
                    self.publish_from_policy(None);
                }
                let _ = reply.send(result);
            }
            Message::CancelAutomaticForConsentRevoke { reply } => {
                cancel_login_preflight(&mut self.login_preflight);
                self.automatic_start_cancelled_by_human = true;
                self.retry = None;
                self.consent_revoke_cancellation = true;
                self.login_terminal_message =
                    Some("登入啟動已由第一張同意書的撤回動作取消。".to_owned());
                self.publish_from_policy(None);
                let _ = reply.send(Ok(()));
            }
            Message::Quit { reply } => {
                let result = self.quit();
                // 呼叫端最多等十秒；若 durable stop 在鎖後面稍晚才成功，原本那個
                // receiver 已消失。這時 worker 自己完成退出，不能永遠卡在 Quitting
                // 等真人再按第二次。
                let should_finish_late = reply_to_quit(reply, result);
                if should_finish_late {
                    self.app.exit(0);
                }
            }
        }
    }

    fn handle_login_start(&mut self) {
        if !take_first_login_intent(&mut self.login_intent_seen) {
            tracing::info!("這個 worker 已承接過登入啟動；忽略 delayed/secondary duplicate");
            return;
        }
        if automatic_login_blocked_by_human(self.automatic_start_cancelled_by_human) {
            self.login_terminal_message = Some(
                "這個 worker 已收到真人停止／撤回；忽略稍後才送到的 Windows 登入啟動。".to_owned(),
            );
            self.publish_from_policy(None);
            return;
        }
        self.login_terminal_message = None;
        let now = Instant::now();
        self.login_preflight = Some(LoginPreflight {
            heartbeat_at: None,
            observed_owner: false,
            retry_at: now,
            expires_at: now + LOGIN_PREFLIGHT_LIMIT,
        });
        self.run_login_attempt();
    }

    fn run_login_attempt(&mut self) {
        match self.attempt_login_start() {
            Ok(()) => {
                self.login_preflight = None;
                self.login_terminal_message = None;
            }
            Err(LoginAttemptError::Retry {
                message,
                heartbeat_at,
                observed_owner,
            }) => self.schedule_login_preflight(message, heartbeat_at, observed_owner),
            Err(LoginAttemptError::External(message)) => {
                self.login_preflight = None;
                self.login_terminal_message = None;
                let transition = policy::reduce(self.policy, policy::Event::ExternalObserved);
                self.policy = transition.state;
                self.retry = None;
                self.publish_from_policy(Some(message));
            }
            Err(LoginAttemptError::Expired) => {
                self.login_preflight = None;
                self.login_terminal_message = Some(
                    "Windows 登入啟動的五分鐘 preflight 已到期；沒有清除停止意圖，也沒有啟動 recorder。"
                        .to_owned(),
                );
                self.publish_from_policy(None);
            }
            Err(LoginAttemptError::Cancel(error)) => {
                self.login_preflight = None;
                tracing::warn!("登入時沒有啟動 recorder：{error}");
                if matches!(
                    self.policy,
                    policy::State::Stopped {
                        reason: policy::StoppedBy::NeverStarted,
                        ..
                    }
                ) {
                    self.login_terminal_message = Some(error);
                    self.publish_from_policy(None);
                }
            }
        }
    }

    fn schedule_login_preflight(
        &mut self,
        message: String,
        observed_heartbeat_at: Option<sister_core::Millis>,
        observed_owner: bool,
    ) {
        let now = Instant::now();
        let pending = self.login_preflight.unwrap_or(LoginPreflight {
            heartbeat_at: observed_heartbeat_at,
            observed_owner,
            retry_at: now,
            expires_at: now + LOGIN_PREFLIGHT_LIMIT,
        });
        match login_retry_decision(self.policy, now < pending.expires_at) {
            LoginRetryDecision::Retry => {
                self.login_preflight = Some(LoginPreflight {
                    heartbeat_at: observed_heartbeat_at.or(pending.heartbeat_at),
                    observed_owner: pending.observed_owner || observed_owner,
                    retry_at: now + LOGIN_PREFLIGHT_RECHECK,
                    expires_at: pending.expires_at,
                });
                self.publish_custom(login_waiting_view(message));
            }
            LoginRetryDecision::Expired => {
                self.login_preflight = None;
                self.login_terminal_message = Some(
                    "Windows 登入啟動的五分鐘 preflight 已到期；沒有清除停止意圖，也沒有啟動 recorder。"
                        .to_owned(),
                );
                self.publish_from_policy(None);
            }
            LoginRetryDecision::Cancelled => {
                self.login_preflight = None;
            }
        }
    }

    fn fire_login_preflight_if_due(&mut self) {
        let Some(pending) = self.login_preflight else {
            return;
        };
        let now = Instant::now();
        match login_retry_decision(self.policy, now < pending.expires_at) {
            LoginRetryDecision::Cancelled => {
                self.login_preflight = None;
                return;
            }
            LoginRetryDecision::Expired => {
                self.login_preflight = None;
                self.login_terminal_message = Some(
                    "Windows 登入啟動的五分鐘 preflight 已到期；沒有清除停止意圖，也沒有啟動 recorder。"
                        .to_owned(),
                );
                self.publish_from_policy(None);
                return;
            }
            LoginRetryDecision::Retry => {}
        }
        if now >= pending.retry_at {
            self.run_login_attempt();
        }
    }

    fn login_heartbeat_for_attempt(
        &self,
        dir: &Path,
        stop_policy: Option<LoginStopPolicy>,
    ) -> Result<
        (
            sister_core::heartbeat::Presence,
            Option<sister_core::Millis>,
        ),
        LoginAttemptError,
    > {
        match login_heartbeat_snapshot(dir) {
            Ok(snapshot) => Ok(snapshot),
            Err(LoginHeartbeatSnapshotError::Changed) => match login_changed_snapshot_decision(stop_policy) {
                // 第一讀已經證明 Absent data dir 有 Live。即使第二讀撞上 tombstone，
                // 這仍是外部 recorder，不可下一拍看到 Stopped 就自動 adopt/restart。
                LoginChangedSnapshotDecision::External => Err(LoginAttemptError::External(
                    "停止控制是 Absent，且第一讀看見 live heartbeat；讀取途中狀態改變也不會把它猜成可自動接管。"
                        .to_owned(),
                )),
                // DesktopQuit 是 narrow handoff；stop.lock 尚忙時則連 typed reason 都
                // 未知。兩者都保留 intent，下一拍從 locked marker 重新判斷。
                LoginChangedSnapshotDecision::Retry => {
                    Err(LoginAttemptError::Retry {
                        message: "recording.beat 在兩次讀取間由 live 換成墓碑／其他狀態；保留 Login intent，下一拍完整重讀"
                            .to_owned(),
                        heartbeat_at: self
                            .login_preflight
                            .and_then(|pending| pending.heartbeat_at),
                        observed_owner: true,
                    })
                }
            },
            Err(LoginHeartbeatSnapshotError::Unknown(message)) => {
                Err(LoginAttemptError::Cancel(message))
            }
        }
    }

    fn attempt_login_start(&mut self) -> Result<(), LoginAttemptError> {
        if automatic_login_blocked_by_human(self.automatic_start_cancelled_by_human) {
            return Err(LoginAttemptError::Cancel(
                "真人已取消 automatic start；登入啟動沒有開始 recorder。".to_owned(),
            ));
        }
        if !matches!(
            self.policy,
            policy::State::Stopped {
                reason: policy::StoppedBy::NeverStarted,
                ..
            }
        ) {
            return Err(LoginAttemptError::Cancel(
                "登入啟動不會覆寫這一輪既有的停止或重試狀態；沒有啟動 recorder".to_owned(),
            ));
        }
        if self.child.is_some() {
            return Err(LoginAttemptError::Cancel(
                "這個 desktop 自己開的 recorder 已經在跑或正在起來".to_owned(),
            ));
        }
        let dir = self
            .data_dir
            .clone()
            .ok_or_else(|| LoginAttemptError::Cancel("找不到資料目錄，開不起來".to_owned()))?;
        // Lock order 固定是 consent(shared) → stop(exclusive) → recorder lease。
        // Busy 代表 revoke/grant writer 已先贏；絕不能先拿 stop 再讀 writer 前快照。
        let recording_consent = match sister_core::consent::try_begin_recording_start(&dir) {
            sister_core::consent::RecordingStartConsent::Allowed(guard) => guard,
            sister_core::consent::RecordingStartConsent::Busy => {
                let (_, heartbeat_at) = self.login_heartbeat_for_attempt(&dir, None)?;
                return Err(LoginAttemptError::Retry {
                    message: "同意書 transaction 正忙；這一拍沒有排在 writer 後面阻塞".to_owned(),
                    heartbeat_at,
                    observed_owner: heartbeat_at.is_some(),
                });
            }
            sister_core::consent::RecordingStartConsent::NotAllowed(_) => {
                return Err(LoginAttemptError::Cancel(
                    "Windows 登入項已執行，但第一張同意書目前無效；沒有開始記錄，也沒有彈出同意書。"
                        .to_owned(),
                ));
            }
            sister_core::consent::RecordingStartConsent::Unknown(error) => {
                return Err(LoginAttemptError::Cancel(format!(
                    "登入啟動無法安全判讀第一張同意書：{error:#}"
                )));
            }
        };
        debug_assert!(recording_consent.belongs_to(&dir));
        let executable = recorder_path().map_err(LoginAttemptError::Cancel)?;
        let transition = policy::reduce(self.policy, policy::Event::StartRequested);
        let policy::Effect::Spawn { generation } = transition.effect else {
            return Err(LoginAttemptError::Cancel(
                "recorder 已經有一個啟動／執行 intent，沒有再開第二個".to_owned(),
            ));
        };

        let start_guard = match sister_core::control::try_begin_start_transaction(&dir) {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                // stop.lock 正忙時還不知道 marker 是 Absent 或 DesktopQuit；此時
                // heartbeat 更新不能先被解讀成 External，只能 bounded retry，等下次
                // 在鎖內取得 typed intent 再決定。
                let (_, heartbeat_at) = self.login_heartbeat_for_attempt(&dir, None)?;
                return Err(LoginAttemptError::Retry {
                    message:
                        "停止控制 transaction 正忙；這一拍沒有排在鎖後面阻塞，也沒有清除停止意圖"
                            .to_owned(),
                    heartbeat_at,
                    observed_owner: heartbeat_at.is_some(),
                });
            }
            Err(error) => {
                return Err(LoginAttemptError::Cancel(format!(
                    "讀不出停止控制 transaction：{error:#}"
                )));
            }
        };
        let stop_policy =
            match login_stop_policy(start_guard.intent().map_err(|error| {
                LoginAttemptError::Cancel(format!("讀不出停止控制狀態：{error:#}"))
            })?) {
                policy @ (LoginStopPolicy::Absent | LoginStopPolicy::DesktopQuit) => policy,
                LoginStopPolicy::Preserve(reason) => {
                    return Err(LoginAttemptError::Cancel(format!(
                        "仍有{}；登入啟動無權清除，沒有啟動 recorder",
                        reason.label()
                    )));
                }
                LoginStopPolicy::Unknown => {
                    return Err(LoginAttemptError::Cancel(
                        "讀不出停止控制狀態；登入啟動沒有猜成可以清除".to_owned(),
                    ));
                }
            };
        if login_observed_owner_blocks_absent(stop_policy, self.login_preflight) {
            return Err(LoginAttemptError::External(
                "停止控制已讀成 Absent，但這份 Login intent 在鎖忙時已觀察到 recorder owner；即使它隨後停止，也不自動接管或重啟。"
                    .to_owned(),
            ));
        }

        let temporary_lease = match sister_core::recorder_lease::try_acquire(&dir) {
            Ok(lease) => lease,
            Err(sister_core::recorder_lease::AcquireError::Occupied { .. }) => {
                let (presence, heartbeat_at) =
                    self.login_heartbeat_for_attempt(&dir, Some(stop_policy))?;
                return match login_occupancy_decision(stop_policy, false, presence) {
                    LoginOccupancyDecision::Retry => Err(LoginAttemptError::Retry {
                        message: "typed DesktopQuit 的上一輪 recorder lease 尚未退場"
                            .to_owned(),
                        heartbeat_at,
                        observed_owner: true,
                    }),
                    LoginOccupancyDecision::External => Err(LoginAttemptError::External(
                        "停止控制是 Absent，但 recorder lease 已被占用；已確認是另一個 recorder，Login 不接管或重啟它。"
                            .to_owned(),
                    )),
                    LoginOccupancyDecision::Start => {
                        unreachable!("an occupied lease cannot authorize login start")
                    }
                };
            }
            Err(error @ sister_core::recorder_lease::AcquireError::Unknown { .. }) => {
                return Err(LoginAttemptError::Cancel(format!(
                    "啟動前無法判定唯一 recorder lease：{error}"
                )));
            }
        };
        let (presence, heartbeat_at) = self.login_heartbeat_for_attempt(&dir, Some(stop_policy))?;
        match login_occupancy_decision(stop_policy, true, presence) {
            LoginOccupancyDecision::Start => {}
            LoginOccupancyDecision::Retry => {
                return Err(LoginAttemptError::Retry {
                    message: sister_core::heartbeat::occupied_why_of(
                        presence,
                        sister_core::now_ms(),
                    )
                    .unwrap_or_else(|| "typed DesktopQuit 的上一輪 heartbeat 尚未退場".to_owned()),
                    heartbeat_at,
                    observed_owner: true,
                });
            }
            LoginOccupancyDecision::External => {
                return Err(LoginAttemptError::External(
                    "停止控制是 Absent，但資料目錄已有 heartbeat／Thinking／stalled 證據；沒有從 mere staleness 猜成可自動接管。"
                        .to_owned(),
                ));
            }
        }
        if !login_commit_before_deadline(self.login_preflight, Instant::now()) {
            return Err(LoginAttemptError::Expired);
        }
        drop(temporary_lease);
        start_guard.clear().map_err(|error| {
            LoginAttemptError::Cancel(format!("清不掉上一次停止意圖，沒有啟動：{error:#}"))
        })?;
        self.policy = transition.state;
        self.publish_from_policy(None);
        with_parent_start_consent(recording_consent, || {
            self.spawn_child(&dir, &executable, generation, StartSource::Login)
                .map_err(LoginAttemptError::Cancel)
        })
    }

    fn start(&mut self, source: StartSource, explicit: Option<&StartReply>) -> Result<(), String> {
        if source != StartSource::Explicit {
            return Err("只有真人 Start 可走新的 start transaction".to_owned());
        }
        if matches!(self.policy, policy::State::Quitting { .. }) {
            return Err("AI-Sister 正在結束，沒有再啟動 recorder".to_owned());
        }
        if self.child.is_some() {
            // 先把 try_wait 的結果和 generation 搬出 borrow，再改 worker 狀態。
            // `child_probe_failed`／`on_child_exit` 都要借整個 self；把它們叫在
            // `self.child.as_mut()` 裡會讓 ownership 規則替這個競爭窗背書失敗。
            let (generation, status) = {
                let spawned = self.child.as_mut().expect("checked above");
                (spawned.generation, spawned.child.try_wait())
            };
            return match status {
                Ok(None) => Err("這個 desktop 自己開的 recorder 已經在跑或正在起來".to_owned()),
                Ok(Some(status)) => {
                    self.child = None;
                    self.on_child_exit(generation, status);
                    Err("上一個 recorder 剛剛才退出；狀態已重新整理，再按一次".to_owned())
                }
                Err(error) => {
                    self.child_probe_failed(generation, error.to_string());
                    Err("問不出上一個 recorder 是否還活著；為避免開出第二個，沒有啟動".to_owned())
                }
            };
        }

        let dir = self
            .data_dir
            .clone()
            .ok_or_else(|| "找不到資料目錄，開不起來".to_owned())?;
        let recording_consent = match sister_core::consent::try_begin_recording_start(&dir) {
            sister_core::consent::RecordingStartConsent::Allowed(guard) => guard,
            sister_core::consent::RecordingStartConsent::Busy => {
                return Err(
                    "同意書 transaction 正忙；真人 Start 沒有等待，也沒有清除停止意圖".to_owned(),
                );
            }
            sister_core::consent::RecordingStartConsent::NotAllowed(_) => {
                let message = "目前讀不到有效的第一張同意書——她不會開始記錄。在系統匣圖示上按右鍵，選「三張同意書…」確認後再回來";
                self.publish_custom(SupervisorView {
                    phase: SupervisorPhase::Stopped,
                    failures: 0,
                    message: Some(message.to_owned()),
                });
                return Err(message.to_owned());
            }
            sister_core::consent::RecordingStartConsent::Unknown(error) => {
                return Err(format!("無法安全判讀第一張同意書：{error:#}"));
            }
        };
        debug_assert!(recording_consent.belongs_to(&dir));
        let executable = recorder_path()?;
        let transition = policy::reduce(self.policy, policy::Event::StartRequested);
        let policy::Effect::Spawn { generation } = transition.effect else {
            return Err("recorder 已經有一個啟動／執行 intent，沒有再開第二個".to_owned());
        };

        let explicit =
            explicit.ok_or_else(|| "真人 Start 缺少同步 commit；沒有啟動 recorder".to_owned())?;
        // 順序固定為 stop transaction → temporary recorder lease →
        // heartbeat/consent barrier → timeout commit → drop lease（仍持 stop lock）→ clear。
        let start_guard = sister_core::control::try_begin_start_transaction(&dir)
            .map_err(|error| format!("讀不出停止控制 transaction：{error:#}"))?
            .ok_or_else(|| {
                "停止控制 transaction 正忙；這次啟動沒有等待，也沒有清除停止意圖".to_owned()
            })?;
        let temporary_lease = sister_core::recorder_lease::try_acquire(&dir)
            .map_err(|error| format!("啟動前無法保留唯一 recorder lease：{error}"))?;
        self.require_vacant(&dir)?;
        if !commit_uncancelled_start(&explicit.commit) {
            return Err("啟動前檢查超過十秒；這次啟動已取消，沒有開 recorder".to_owned());
        }
        // Child 仍一律是 Supervised，絕不能在稍後清掉較新的 Stop／Quit。
        drop(temporary_lease);
        start_guard
            .clear()
            .map_err(|error| format!("清不掉上一次停止意圖，沒有啟動：{error:#}"))?;
        self.policy = transition.state;
        self.publish_from_policy(None);
        let spawned = with_parent_start_consent(recording_consent, || {
            self.spawn_child(&dir, &executable, generation, source)
        });
        if commit_human_explicit_start_if_spawned(
            &spawned,
            &mut self.automatic_start_cancelled_by_human,
            &mut self.stop_delivery_failure,
        ) {
            // 只有 OS 確認 Command::spawn 成功，才算真人建立了新的 recording
            // intent，可以取代先前「即使 durable Stop 寫失敗也禁用自動復活」
            // 的 in-memory latch。只清掉舊 stop、接著 spawn 失敗仍是 failed
            // Explicit，不能偷偷讓它進 watchdog automatic retry。
            self.consent_revoke_cancellation = false;
            self.publish_from_policy(None);
        }
        spawned
    }

    fn require_vacant(&self, dir: &Path) -> Result<(), String> {
        let now = sister_core::now_ms();
        match sister_core::heartbeat::presence(dir, now) {
            sister_core::heartbeat::Presence::Unreadable => Err(
                "讀不懂 recording.beat；為避免同一個資料目錄同時開兩個 recorder，沒有啟動"
                    .to_owned(),
            ),
            presence => sister_core::heartbeat::occupied_why_of(presence, now).map_or(Ok(()), Err),
        }
    }

    fn spawn_child(
        &mut self,
        dir: &Path,
        executable: &Path,
        generation: policy::Generation,
        source: StartSource,
    ) -> Result<(), String> {
        let result: Result<Spawned, String> = (|| {
            let (out, err) = record_log(dir, source)?;
            let mut command = Command::new(executable);
            command
                .arg("--data-dir")
                .arg(dir)
                .arg("record")
                .arg(SUPERVISED_ARGUMENT)
                .stdin(Stdio::null())
                .stdout(Stdio::from(out))
                .stderr(Stdio::from(err));
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let at = sister_core::now_ms();
            let child = command
                .spawn()
                .map_err(|error| format!("{} 起不來：{error}", executable.display()))?;
            Ok(Spawned {
                child,
                at,
                generation,
            })
        })();

        match result {
            Ok(spawned) => {
                let pid = spawned.child.id();
                self.child = Some(spawned);
                self.owned_child_observed_gone_at = None;
                self.last_observed_heartbeat = None;
                let transition =
                    policy::reduce(self.policy, policy::Event::SpawnSucceeded { generation });
                self.policy = transition.state;
                self.retry = None;
                self.last_error = None;
                tracing::info!(
                    "把 supervised recorder 開起來了：pid {pid}；{}",
                    executable.display()
                );
                self.publish_from_policy(None);
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                self.owned_child_observed_gone_at = Some(sister_core::now_ms());
                let transition = policy::reduce(
                    self.policy,
                    policy::Event::SpawnFailed {
                        generation,
                        at: self.now(),
                    },
                );
                self.apply_transition(transition);
                Err(error)
            }
        }
    }

    fn poll_child(&mut self) {
        let Some(spawned) = self.child.as_mut() else {
            return;
        };
        match spawned.child.try_wait() {
            Ok(None) => {
                let generation = spawned.generation;
                let recovered = policy::reduce(
                    self.policy,
                    policy::Event::ChildProbeSucceeded { generation },
                );
                if recovered.state != self.policy {
                    self.policy = recovered.state;
                    self.publish_from_policy(None);
                }
                self.observe_phase(generation);
            }
            Ok(Some(status)) => {
                let generation = spawned.generation;
                self.child = None;
                self.on_child_exit(generation, status);
            }
            Err(error) => {
                let generation = spawned.generation;
                self.child_probe_failed(generation, error.to_string());
            }
        }
    }

    fn observe_phase(&mut self, generation: policy::Generation) {
        let Some(dir) = self.data_dir.as_deref() else {
            return;
        };
        let observed = sister_core::heartbeat::live_heartbeat(dir, sister_core::now_ms());
        let phase = match observed {
            Some(sample) if self.last_observed_heartbeat == Some(sample) => return,
            Some((_, sister_core::heartbeat::Phase::Booting)) => policy::RecorderPhase::Booting,
            Some((_, sister_core::heartbeat::Phase::Recording)) => policy::RecorderPhase::Recording,
            // Child::try_wait 說它還活著，但這一拍沒有一份新鮮 Recording
            // heartbeat。這不一定是 crash，所以不動 failure budget；但它一定
            // 會打斷「連續 Recording 十分鐘」，不能等下次心跳回來後接著算。
            None => policy::RecorderPhase::NotRecording,
        };
        self.last_observed_heartbeat = observed;
        let before = self.policy;
        self.policy = policy::reduce(
            self.policy,
            policy::Event::PhaseObserved {
                generation,
                phase,
                at: self.now(),
            },
        )
        .state;
        if self.policy != before {
            self.publish_from_policy(None);
        }
    }

    fn on_child_exit(&mut self, generation: policy::Generation, status: ExitStatus) {
        // 先立 cutoff；後面的 stop/consent/exit 分支不論走哪一條，最後一顆 owned
        // heartbeat 都不能再被 UI 當成「目前仍在錄」。
        self.owned_child_observed_gone_at = Some(sister_core::now_ms());
        let detail = exit_detail(status);
        tracing::warn!("supervised recorder 已退出：{detail}");
        let control_probe = self.cancellation_from_disk();
        if let Ok(Some(reason)) = control_probe.as_ref() {
            self.stop_policy(*reason);
            return;
        }
        if status.success() {
            let transition = policy::reduce(
                self.policy,
                policy::Event::ChildExited {
                    generation,
                    exit: policy::ChildExit::Success,
                    at: self.now(),
                },
            );
            self.policy = transition.state;
            self.retry = None;
            let message = if control_probe.is_err() {
                "record 已正常收工；雖然停止控制狀態讀不出來，exit 0 本身已證明不需自動重試。"
            } else {
                "record 已正常收工，不會自動重試。"
            };
            self.publish_from_policy(Some(message.to_owned()));
            return;
        }

        self.last_error = Some(detail);
        let transition = policy::reduce(
            self.policy,
            policy::Event::ChildExited {
                generation,
                exit: policy::ChildExit::Failure,
                at: self.now(),
            },
        );
        if control_probe.is_err() {
            self.policy = transition.state;
            let unknown = policy::reduce(
                self.policy,
                policy::Event::RetryDue {
                    generation,
                    at: self.now(),
                    readiness: policy::RetryReadiness::Unknown,
                },
            );
            self.apply_transition(unknown);
        } else {
            self.apply_transition(transition);
        }
    }

    /// `Ok(None)` 只代表這一拍確定沒有 stop intent 且 consent 仍有效；任何讀取
    /// 不確定都留成 `Err`，呼叫端不得把它猜成「可以 retry」。
    fn cancellation_from_disk(&self) -> Result<Option<policy::StoppedBy>, String> {
        let Some(dir) = self.data_dir.as_deref() else {
            return Err("找不到資料目錄，讀不到停止控制狀態".to_owned());
        };
        let reason = match sister_core::control::stop_intent(dir) {
            sister_core::control::StopIntent::Pending(reason)
            | sister_core::control::StopIntent::Consumed(reason) => match reason {
                sister_core::control::StopReason::DesktopQuit
                | sister_core::control::StopReason::Requested => policy::StoppedBy::Requested,
                sister_core::control::StopReason::ConsentRevoked => {
                    policy::StoppedBy::ConsentRevoked
                }
            },
            sister_core::control::StopIntent::Absent
                if !sister_core::consent::load(dir).allows_recording() =>
            {
                policy::StoppedBy::ConsentRevoked
            }
            sister_core::control::StopIntent::Absent => return Ok(None),
            sister_core::control::StopIntent::Uncheckable => {
                return Err("讀不出停止控制狀態".to_owned());
            }
        };
        Ok(Some(reason))
    }

    fn poll_stop_intent(&mut self) {
        if !matches!(
            self.policy,
            policy::State::Starting { .. }
                | policy::State::Running { .. }
                | policy::State::ChildUncertain { .. }
                | policy::State::Backoff { .. }
                | policy::State::External { .. }
                | policy::State::ExternalStopping { .. }
        ) {
            return;
        }
        match self.cancellation_from_disk() {
            Ok(Some(reason)) => self.stop_policy(reason),
            Ok(None) => self.publish_from_policy(None),
            Err(_) if self.stop_delivery_pending || self.stop_delivery_failure.is_some() => {
                // 這份 overlay 是真人 Stop 的 delivery truth。後續讀不到 stop.lock
                // 不能把它洗成一般 Uncertain，否則系統匣會失去安全的重送入口。
                self.publish_from_policy(None);
            }
            Err(_error) if matches!(self.policy, policy::State::Backoff { .. }) => {
                let generation = self.policy.generation();
                let transition = policy::reduce(
                    self.policy,
                    policy::Event::RetryDue {
                        generation,
                        at: self.now(),
                        readiness: policy::RetryReadiness::Unknown,
                    },
                );
                self.apply_transition(transition);
            }
            Err(error) => self.publish_custom(SupervisorView {
                phase: SupervisorPhase::Uncertain,
                failures: self.policy.failures().get(),
                message: Some(format!(
                    "{error}；目前不能確認 recorder 是否收到停止要求，也不會另開一份。"
                )),
            }),
        }
    }

    fn stop(&mut self, reason: policy::StoppedBy) -> Result<(), String> {
        let dir = self
            .data_dir
            .as_deref()
            .ok_or_else(|| "找不到資料目錄，停不了".to_owned())?;
        let written = match reason {
            policy::StoppedBy::Requested => sister_core::control::request_stop(dir),
            policy::StoppedBy::ConsentRevoked => sister_core::control::request_consent_revoke(dir),
            policy::StoppedBy::NeverStarted
            | policy::StoppedBy::SuccessfulExit
            | policy::StoppedBy::ExternalExit => {
                unreachable!("only explicit cancellation reaches Worker::stop")
            }
        };
        written.map_err(|error| format!("{error:#}"))?;
        self.stop_policy(reason);
        Ok(())
    }

    fn stop_policy(&mut self, reason: policy::StoppedBy) {
        self.login_preflight = None;
        self.login_terminal_message = None;
        self.stop_delivery_pending = false;
        self.stop_delivery_failure = None;
        self.consent_revoke_cancellation = false;
        let event = match reason {
            policy::StoppedBy::Requested => policy::Event::StopRequested,
            policy::StoppedBy::ConsentRevoked => policy::Event::ConsentRevoked,
            policy::StoppedBy::NeverStarted
            | policy::StoppedBy::SuccessfulExit
            | policy::StoppedBy::ExternalExit => {
                unreachable!("not a cancellation event")
            }
        };
        let transition = policy::reduce(self.policy, event);
        self.policy = transition.state;
        self.retry = None;
        let message = match reason {
            policy::StoppedBy::Requested => "已請 recorder 收工；不會被自動重試叫回來。",
            policy::StoppedBy::ConsentRevoked => {
                "本機記錄同意的停止條件已生效；recorder 不會被自動重試叫回來。"
            }
            policy::StoppedBy::NeverStarted
            | policy::StoppedBy::SuccessfulExit
            | policy::StoppedBy::ExternalExit => unreachable!(),
        };
        self.publish_from_policy(Some(message.to_owned()));
    }

    fn detached_heartbeat(&self) -> DetachedHeartbeat {
        if self.child.is_some()
            || matches!(
                self.policy,
                policy::State::Starting { .. }
                    | policy::State::Running { .. }
                    | policy::State::ChildUncertain { .. }
                    | policy::State::Quitting { .. }
            )
        {
            return DetachedHeartbeat::Vacant;
        }
        let Some(dir) = self.data_dir.as_deref() else {
            return DetachedHeartbeat::Unreadable;
        };
        let presence = sister_core::heartbeat::presence(dir, sister_core::now_ms());
        let external_state = matches!(
            self.policy,
            policy::State::External { .. } | policy::State::ExternalStopping { .. }
        );
        match presence {
            sister_core::heartbeat::Presence::Unreadable => {
                return DetachedHeartbeat::Unreadable;
            }
            sister_core::heartbeat::Presence::Stopped { .. } if external_state => {
                return DetachedHeartbeat::ExternalGone;
            }
            sister_core::heartbeat::Presence::NeverStarted if external_state => {
                return DetachedHeartbeat::ExternalUncertain;
            }
            sister_core::heartbeat::Presence::Stalled { at, .. }
                if external_state
                    || policy::heartbeat_is_external(
                        policy::OwnedChildGoneAt::new(self.owned_child_observed_gone_at),
                        policy::ObservedHeartbeatAt::new(Some(at)),
                    ) =>
            {
                // 不新鮮不等於已退出。尤其舊版 recorder 沒有 recording.lock；一拍
                // 若在 owned cutoff 之後出現，就算它目前 suspended 也不能重開。
                return DetachedHeartbeat::ExternalUncertain;
            }
            sister_core::heartbeat::Presence::NeverStarted
            | sister_core::heartbeat::Presence::Stopped { .. }
            | sister_core::heartbeat::Presence::Stalled { .. } => {
                return DetachedHeartbeat::Vacant;
            }
            sister_core::heartbeat::Presence::Live(_)
            | sister_core::heartbeat::Presence::Thinking { .. } => {}
        }

        if external_state {
            return DetachedHeartbeat::External;
        }
        let external = policy::heartbeat_is_external(
            policy::OwnedChildGoneAt::new(self.owned_child_observed_gone_at),
            policy::ObservedHeartbeatAt::new(sister_core::heartbeat::last_beat(dir)),
        );
        if external {
            DetachedHeartbeat::External
        } else {
            DetachedHeartbeat::OldOwned
        }
    }

    /// 沒有 Child handle 時仍每拍觀察 heartbeat：外部 recorder 可以在第四次
    /// failure 之後才出現，也可以自行收工。觀察不等於接管；External state 永遠
    /// 不 spawn、不 kill，也不恢復舊 retry。
    fn poll_detached_heartbeat(&mut self) {
        if self.login_preflight.is_some()
            || self.child.is_some()
            || matches!(
                self.policy,
                policy::State::Starting { .. }
                    | policy::State::Running { .. }
                    | policy::State::ChildUncertain { .. }
                    | policy::State::Quitting { .. }
            )
        {
            return;
        }

        match self.detached_heartbeat() {
            DetachedHeartbeat::External => {
                let transition = policy::reduce(self.policy, policy::Event::ExternalObserved);
                if transition.state != self.policy {
                    self.apply_transition(transition);
                } else {
                    self.publish_from_policy(None);
                }
            }
            DetachedHeartbeat::ExternalGone
                if matches!(
                    self.policy,
                    policy::State::External { .. } | policy::State::ExternalStopping { .. }
                ) =>
            {
                self.apply_transition(policy::reduce(self.policy, policy::Event::ExternalGone));
            }
            DetachedHeartbeat::ExternalUncertain => {
                let transition = policy::reduce(self.policy, policy::Event::ExternalObserved);
                if transition.state != self.policy {
                    self.apply_transition(transition);
                } else {
                    self.publish_from_policy(None);
                }
            }
            DetachedHeartbeat::Unreadable
                if self.stop_delivery_pending || self.stop_delivery_failure.is_some() =>
            {
                self.publish_from_policy(None);
            }
            DetachedHeartbeat::Unreadable
                if matches!(self.policy, policy::State::Backoff { .. }) =>
            {
                let generation = self.policy.generation();
                self.apply_transition(policy::reduce(
                    self.policy,
                    policy::Event::RetryDue {
                        generation,
                        at: self.now(),
                        readiness: policy::RetryReadiness::Unknown,
                    },
                ));
            }
            DetachedHeartbeat::Unreadable => self.publish_custom(SupervisorView {
                phase: SupervisorPhase::Uncertain,
                failures: self.policy.failures().get(),
                message: Some(
                    "讀不懂 recording.beat；目前不能確認是否有外部 recorder，沒有嘗試接管。"
                        .to_owned(),
                ),
            }),
            DetachedHeartbeat::OldOwned
            | DetachedHeartbeat::Vacant
            | DetachedHeartbeat::ExternalGone => self.publish_from_policy(None),
        }
    }

    fn child_probe_failed(&mut self, generation: policy::Generation, error: String) {
        self.last_error = Some(format!("問不出 recorder child 狀態：{error}"));
        let transition =
            policy::reduce(self.policy, policy::Event::ChildProbeFailed { generation });
        self.apply_transition(transition);
    }

    fn fire_retry_if_due(&mut self) {
        if self.automatic_start_cancelled_by_human {
            self.retry = None;
            return;
        }
        let Some((generation, due)) = self.retry else {
            return;
        };
        if Instant::now() < due {
            return;
        }
        let (readiness, barrier_message, start_barrier) = self.retry_start_readiness();
        let transition = policy::reduce(
            self.policy,
            policy::Event::RetryDue {
                generation,
                at: self.now(),
                readiness,
            },
        );
        self.policy = transition.state;
        match transition.effect {
            policy::Effect::Spawn { generation } => {
                self.retry = None;
                self.publish_from_policy(None);
                let Some(dir) = self.data_dir.clone() else {
                    self.give_up_probe("重試時找不到資料目錄");
                    return;
                };
                let executable = match recorder_path() {
                    Ok(path) => path,
                    Err(error) => {
                        self.last_error = Some(error.clone());
                        let failed = policy::reduce(
                            self.policy,
                            policy::Event::SpawnFailed {
                                generation,
                                at: self.now(),
                            },
                        );
                        self.apply_transition(failed);
                        return;
                    }
                };
                let Some(start_barrier) = start_barrier else {
                    self.give_up_probe(
                        "watchdog 缺少 held consent/stop/lease start transaction；沒有重開 recorder",
                    );
                    return;
                };
                let RetryStartBarrier {
                    consent,
                    stop,
                    lease,
                } = start_barrier;
                // Child 要自己取得 recording.lock，所以 spawn 前釋放 temporary lease；
                // 但仍持 consent shared + stop exclusive，讓任何 Stop/revoke 和這次
                // watchdog start 有單一、可解釋的先後順序。
                drop(lease);
                let _ = with_parent_start_consent(consent, || {
                    self.spawn_child(&dir, &executable, generation, StartSource::Retry)
                });
                drop(stop);
            }
            policy::Effect::ScheduleRetry { generation, after } => {
                self.retry = Some((
                    generation,
                    Instant::now() + Duration::from_millis(after.as_millis()),
                ));
                self.publish_from_policy(barrier_message);
            }
            policy::Effect::CancelRetry => {
                self.retry = None;
                self.publish_from_policy(None);
            }
            policy::Effect::GaveUp { .. } | policy::Effect::None => {
                self.retry = None;
                self.publish_from_policy(barrier_message);
            }
        }
    }

    fn retry_start_readiness(
        &mut self,
    ) -> (
        policy::RetryReadiness,
        Option<String>,
        Option<RetryStartBarrier>,
    ) {
        let Some(dir) = self.data_dir.clone() else {
            return (
                policy::RetryReadiness::Unknown,
                Some("重試時找不到資料目錄。".to_owned()),
                None,
            );
        };
        let consent = match sister_core::consent::try_begin_recording_start(&dir) {
            sister_core::consent::RecordingStartConsent::Allowed(guard) => guard,
            sister_core::consent::RecordingStartConsent::Busy => {
                return (
                    policy::RetryReadiness::Occupied {
                        retry_after: OCCUPIED_RECHECK,
                    },
                    Some(
                        "同意書 transaction 正忙；watchdog 沒有讀 writer 前快照，也沒有重開 recorder。"
                            .to_owned(),
                    ),
                    None,
                );
            }
            sister_core::consent::RecordingStartConsent::NotAllowed(_) => {
                return (policy::RetryReadiness::ConsentRevoked, None, None);
            }
            sister_core::consent::RecordingStartConsent::Unknown(error) => {
                return (
                    policy::RetryReadiness::Unknown,
                    Some(format!("重試時無法安全判讀第一張同意書：{error:#}")),
                    None,
                );
            }
        };
        debug_assert!(consent.belongs_to(&dir));
        let stop = match sister_core::control::try_begin_start_transaction(&dir) {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                return (
                    policy::RetryReadiness::Occupied {
                        retry_after: OCCUPIED_RECHECK,
                    },
                    Some(
                        "停止控制 transaction 正忙；watchdog 沒有排在鎖後面，也沒有重開 recorder。"
                            .to_owned(),
                    ),
                    None,
                );
            }
            Err(error) => {
                return (
                    policy::RetryReadiness::Unknown,
                    Some(format!("讀不出停止控制 transaction：{error:#}")),
                    None,
                );
            }
        };
        match stop.intent() {
            Ok(sister_core::control::StopIntent::Absent) => {}
            Ok(
                sister_core::control::StopIntent::Pending(reason)
                | sister_core::control::StopIntent::Consumed(reason),
            ) => {
                let readiness = match reason {
                    sister_core::control::StopReason::DesktopQuit
                    | sister_core::control::StopReason::Requested => {
                        policy::RetryReadiness::StopRequested
                    }
                    sister_core::control::StopReason::ConsentRevoked => {
                        policy::RetryReadiness::ConsentRevoked
                    }
                };
                return (readiness, None, None);
            }
            Ok(sister_core::control::StopIntent::Uncheckable) | Err(_) => {
                return (
                    policy::RetryReadiness::Unknown,
                    Some("讀不出停止控制狀態；沒有冒險重開 recorder。".to_owned()),
                    None,
                );
            }
        }
        let lease = match sister_core::recorder_lease::try_acquire(&dir) {
            Ok(lease) => lease,
            Err(sister_core::recorder_lease::AcquireError::Occupied { .. }) => {
                return (
                    policy::RetryReadiness::Occupied {
                        retry_after: OCCUPIED_RECHECK,
                    },
                    Some("recording.lock 仍被另一個 recorder 占用；沒有重開。".to_owned()),
                    None,
                );
            }
            Err(error @ sister_core::recorder_lease::AcquireError::Unknown { .. }) => {
                return (
                    policy::RetryReadiness::Unknown,
                    Some(format!("無法安全判定 recording.lock：{error}")),
                    None,
                );
            }
        };
        let (readiness, message) = self.retry_heartbeat_readiness(&dir);
        let barrier =
            matches!(readiness, policy::RetryReadiness::Ready).then_some(RetryStartBarrier {
                consent,
                stop,
                lease,
            });
        (readiness, message, barrier)
    }

    fn retry_heartbeat_readiness(
        &mut self,
        dir: &Path,
    ) -> (policy::RetryReadiness, Option<String>) {
        let now = sister_core::now_ms();
        match sister_core::heartbeat::presence(dir, now) {
            sister_core::heartbeat::Presence::Unreadable => (
                policy::RetryReadiness::Unknown,
                Some("讀不懂 recording.beat；沒有冒險重開第二個 recorder。".to_owned()),
            ),
            sister_core::heartbeat::Presence::Stalled { at, .. }
                if policy::heartbeat_is_external(
                    policy::OwnedChildGoneAt::new(self.owned_child_observed_gone_at),
                    policy::ObservedHeartbeatAt::new(Some(at)),
                ) =>
            {
                (
                    policy::RetryReadiness::External,
                    Some(
                        "owned child 退出後有另一份 recorder 蓋過 heartbeat；目前雖已不新鮮，仍不能證明它不會恢復，沒有重開。"
                            .to_owned(),
                    ),
                )
            }
            presence if sister_core::heartbeat::occupied_of(presence) => {
                // 已退出的 child 不可能在我們看見它退出之後繼續蓋拍。若時戳又往
                // 後走，現在佔著的是另一個 recorder；停在這裡，不去接管它。
                let external = policy::heartbeat_is_external(
                    policy::OwnedChildGoneAt::new(self.owned_child_observed_gone_at),
                    policy::ObservedHeartbeatAt::new(sister_core::heartbeat::last_beat(dir)),
                );
                if external {
                    (
                        policy::RetryReadiness::External,
                        Some(
                            "另一個 recorder 已開始更新同一個資料目錄；這輪 supervisor 不接管它。"
                                .to_owned(),
                        ),
                    )
                } else {
                    (
                        policy::RetryReadiness::Occupied {
                            retry_after: OCCUPIED_RECHECK,
                        },
                        Some(
                            "舊心跳仍在安全期限內；先不開第二個 recorder，等它過期後再重試。"
                                .to_owned(),
                        ),
                    )
                }
            }
            sister_core::heartbeat::Presence::NeverStarted
            | sister_core::heartbeat::Presence::Stopped { .. }
            | sister_core::heartbeat::Presence::Stalled { .. } => {
                (policy::RetryReadiness::Ready, None)
            }
            sister_core::heartbeat::Presence::Live(_)
            | sister_core::heartbeat::Presence::Thinking { .. } => {
                unreachable!("occupied arm handled above")
            }
        }
    }

    fn give_up_probe(&mut self, message: &str) {
        self.retry = None;
        self.publish_custom(SupervisorView {
            phase: SupervisorPhase::Uncertain,
            failures: self.policy.failures().get(),
            message: Some(message.to_owned()),
        });
    }

    fn apply_transition(&mut self, transition: policy::Transition) {
        self.policy = transition.state;
        if automatic_effect_blocked_by_human(
            self.automatic_start_cancelled_by_human,
            transition.effect,
        ) {
            self.retry = None;
            self.publish_from_policy(None);
            return;
        }
        match transition.effect {
            policy::Effect::ScheduleRetry { generation, after } => {
                self.retry = Some((
                    generation,
                    Instant::now() + Duration::from_millis(after.as_millis()),
                ));
            }
            policy::Effect::CancelRetry | policy::Effect::GaveUp { .. } | policy::Effect::None => {
                self.retry = None
            }
            policy::Effect::Spawn { .. } => {
                unreachable!("spawn effects are handled by their request/retry call site")
            }
        }
        self.publish_from_policy(None);
    }

    fn publish_from_policy(&self, override_message: Option<String>) {
        if matches!(self.policy, policy::State::Quitting { .. }) {
            if let Some(error) = self.quit_failure.as_deref() {
                self.publish_custom(SupervisorView {
                    phase: SupervisorPhase::Uncertain,
                    failures: self.policy.failures().get(),
                    message: Some(format!(
                        "{error}。AI-Sister 沒有退出，因為無法證明 recorder 會停下來。"
                    )),
                });
                return;
            }
        }
        if self.stop_delivery_pending {
            self.publish_custom(stop_delivery_pending_view(self.policy));
            return;
        }
        if let Some(error) = self.stop_delivery_failure.as_deref() {
            self.publish_custom(stop_delivery_failure_view(
                self.policy,
                self.child.is_some(),
                error,
            ));
            return;
        }
        if self.consent_revoke_cancellation {
            self.publish_custom(consent_revoke_cancellation_view(
                self.policy,
                self.child.is_some(),
            ));
            return;
        }
        let login_terminal_message = retained_login_terminal_message(
            self.policy,
            override_message.is_some(),
            self.login_terminal_message.as_deref(),
        )
        .map(str::to_owned);
        let failures = self.policy.failures().get();
        let (phase, message) = match self.policy {
            policy::State::Stopped { reason, .. } => {
                let default = match reason {
                    policy::StoppedBy::NeverStarted => None,
                    policy::StoppedBy::SuccessfulExit => {
                        Some("record 已正常收工，不會自動重試。".to_owned())
                    }
                    policy::StoppedBy::ExternalExit => {
                        Some(
                            "已讀到另一個 recorder 留下的停止墓碑；desktop 沒有接管或重啟它。"
                                .to_owned(),
                        )
                    }
                    policy::StoppedBy::Requested => {
                        Some("已請 recorder 收工；不會被自動重試叫回來。".to_owned())
                    }
                    policy::StoppedBy::ConsentRevoked => {
                        Some(
                            "本機記錄同意的停止條件已生效；recorder 不會被自動重試叫回來。"
                                .to_owned(),
                        )
                    }
                };
                (SupervisorPhase::Stopped, default)
            }
            policy::State::Starting { .. } => (
                SupervisorPhase::Starting,
                Some("正在啟動 recorder；這期間還沒有開始記錄。".to_owned()),
            ),
            policy::State::Running { .. } => (SupervisorPhase::Running, None),
            policy::State::ChildUncertain { .. } => (
                SupervisorPhase::Uncertain,
                Some(
                    "剛剛問不出 owned recorder child 是否仍活著；保留原 Child handle，沒有另開第二份。"
                        .to_owned(),
                ),
            ),
            policy::State::Backoff { failures, .. } => {
                let delay = match failures.get() {
                    1 => 1,
                    2 => 5,
                    3 => 30,
                    _ => 0,
                };
                let why = self.last_error.as_deref().unwrap_or("沒有 exit code");
                (
                    SupervisorPhase::Backoff,
                    Some(format!(
                        "record 剛剛異常退出（{why}）。desktop 仍在；{delay} 秒後做第 {}/3 次自動重試。",
                        failures.get()
                    )),
                )
            }
            policy::State::GaveUp { cause, .. } => {
                match cause {
                    policy::GaveUpCause::FourFailures => (
                        SupervisorPhase::GaveUp,
                        Some(
                            "record 連續失敗 4 次，已停止自動重試；從現在起發生的事她不會知道。按「再試一次」才會重新開始。".to_owned(),
                        ),
                    ),
                    // Policy 的 GaveUp 只表示 timer 已取消；ProbeUnknown 沒有證明
                    // child 已停。投影成「放棄／再試」會把不確定講成已停止，甚至
                    // 鼓勵真人另開一份。UI 必須停在不能操作的 Uncertain。
                    policy::GaveUpCause::ProbeUnknown => (
                        SupervisorPhase::Uncertain,
                        Some(override_message.clone().unwrap_or_else(|| {
                            "問不出 recorder 是否能安全重開；已停止自動重試，沒有冒險開第二個。"
                                .to_owned()
                        })),
                    ),
                }
            }
            policy::State::External { .. } => (
                SupervisorPhase::External,
                Some(
                    "最近一拍辨識為另一個 recorder；desktop 只觀察，不接管或替它重試。"
                        .to_owned(),
                ),
            ),
            policy::State::ExternalStopping { reason, .. } => (
                SupervisorPhase::StoppingExternal,
                Some(
                    match reason {
                        policy::StoppedBy::Requested => {
                            "已留下停止外部 recorder 的要求；正在等它寫出停止墓碑，desktop 不接管或重啟它。"
                        }
                        policy::StoppedBy::ConsentRevoked => {
                            "本機記錄同意的停止條件已生效；正在等外部 recorder 寫出停止墓碑，desktop 不接管或重啟它。"
                        }
                        policy::StoppedBy::NeverStarted
                        | policy::StoppedBy::SuccessfulExit
                        | policy::StoppedBy::ExternalExit => {
                            unreachable!("external stopping only has cancellation reasons")
                        }
                    }
                    .to_owned(),
                ),
            ),
            policy::State::Quitting { .. } => (
                SupervisorPhase::Quitting,
                Some("AI-Sister 正在結束；不會再啟動 recorder。".to_owned()),
            ),
        };
        let message = override_message.or(message);
        let (phase, message) = match self.detached_heartbeat() {
            DetachedHeartbeat::External if phase == SupervisorPhase::StoppingExternal => {
                (phase, message)
            }
            DetachedHeartbeat::External => (
                SupervisorPhase::External,
                Some(
                    "最近一拍辨識為另一個 recorder；desktop 只觀察，不接管或替它重試。"
                        .to_owned(),
                ),
            ),
            DetachedHeartbeat::OldOwned if phase == SupervisorPhase::Stopped => (
                SupervisorPhase::Cooling,
                Some(
                    "owned recorder 已退出；正在等那一輪最後的 heartbeat 證據退場，沒有開放重新啟動。"
                        .to_owned(),
                ),
            ),
            DetachedHeartbeat::ExternalUncertain => (
                SupervisorPhase::Uncertain,
                Some(if phase == SupervisorPhase::StoppingExternal {
                    "已留下停止外部 recorder 的要求，但目前沒有新鮮心跳也沒有停止墓碑；不能確認它是否已收工，沒有開放重新啟動。"
                        .to_owned()
                } else {
                    "最近觀察到外部 recorder，但目前沒有新鮮心跳也沒有停止墓碑；不能確認它是否已收工，沒有開放重新啟動。"
                        .to_owned()
                }),
            ),
            DetachedHeartbeat::Unreadable => (
                SupervisorPhase::Uncertain,
                Some(
                    "讀不懂 recording.beat；目前不能確認是否有外部 recorder，沒有嘗試接管。"
                        .to_owned(),
                ),
            ),
            DetachedHeartbeat::Vacant
            | DetachedHeartbeat::OldOwned
            | DetachedHeartbeat::ExternalGone => (phase, message),
        };
        self.publish_custom(SupervisorView {
            phase,
            failures,
            message: login_terminal_message.or(message),
        });
    }

    fn publish_custom(&self, view: SupervisorView) {
        let mut current = self.published.lock().expect("recorder supervisor view");
        if *current == view {
            return;
        }
        *current = view.clone();
        drop(current);
        let _ = self.app.emit(CHANGED_EVENT, view);
    }

    fn quit(&mut self) -> Result<(), String> {
        // 先讓所有 generation/timer 失效，再碰可能需要等另一個 writer 的 durable
        // control lock。反過來排的話，quit 呼叫雖然十秒後 timeout，worker 卻仍
        // 卡在寫檔前、舊 retry 也仍有效；畫面消失與否就會改變是否復活 recorder。
        self.policy = policy::reduce(self.policy, policy::Event::Quit).state;
        self.retry = None;
        self.login_preflight = None;
        self.automatic_start_cancelled_by_human = true;
        self.consent_revoke_cancellation = false;
        self.quit_failure = None;
        self.publish_from_policy(None);

        // data_dir 建不出來時，這個 worker 從來不可能 spawn child；沒有 owned
        // recorder 或 retry 可留在背後，不能因此把空殼 desktop 困住。有正常
        // data_dir 時仍寫 stop：tray 可能正顯示一個外部 recorder，且明講 quit
        // 會讓那份記錄停下來。
        if self.child.is_none() && self.data_dir.is_none() {
            self.quit_confirmed.store(true, Ordering::Release);
            self.done = true;
            return Ok(());
        }

        let write_result = self
            .data_dir
            .as_deref()
            .ok_or_else(|| "結束時找不到資料目錄，停不了 recorder".to_owned())
            .and_then(|dir| {
                sister_core::control::request_desktop_quit(dir)
                    .map_err(|error| format!("結束時寫不進停止請求：{error:#}"))
            });

        // durable write 失敗時，這個還活著的 desktop 同樣不能消失，把一個已開
        // DB、不能安全 kill 的 child 留在背後繼續錄。上面的 Quitting state 已先
        // 拒絕新 start／retry；真人修好磁碟或權限後可以再按一次 quit。
        if let Err(error) = write_result {
            self.quit_failure = Some(error.clone());
            self.publish_from_policy(None);
            return Err(error);
        }

        if let (Some(dir), Some(spawned)) = (self.data_dir.as_deref(), self.child.as_mut()) {
            let still_running = matches!(spawned.child.try_wait(), Ok(None));
            if still_running
                && sister_core::heartbeat::safe_to_kill_spawn(
                    dir,
                    spawned.at,
                    sister_core::now_ms(),
                )
            {
                match spawned.child.kill() {
                    Ok(()) => {
                        tracing::info!(
                            "剛 spawn 出來還沒開資料庫，直接收掉：pid {}",
                            spawned.child.id()
                        );
                        let _ = spawned.child.wait();
                    }
                    Err(error) => tracing::error!("收不掉剛 spawn 的 recorder：{error}"),
                }
            }
        }
        self.quit_confirmed.store(true, Ordering::Release);
        self.done = true;
        Ok(())
    }

    fn handle_disconnected(&mut self) {
        // App state 已經在拆；即使無人等回條，也先留下 durable stop，再讓 Child
        // handle drop。drop 不會 kill，所以 recorder 仍能自行 finalize。
        let _ = self.quit();
    }
}

pub(crate) fn recorder_path() -> Result<PathBuf, String> {
    let me = std::env::current_exe().map_err(|error| format!("問不出自己在哪裡：{error}"))?;
    let directory = me
        .parent()
        .ok_or_else(|| "問不出自己在哪個資料夾".to_owned())?;
    let name = if cfg!(windows) {
        "sister.exe"
    } else {
        "sister"
    };
    let path = directory.join(name);
    match path.try_exists() {
        Ok(true) => Ok(path),
        _ => Err(format!(
            "找不到 {name}——它應該和 sister-desktop 放在同一個資料夾（{}）",
            directory.display()
        )),
    }
}

fn record_log(dir: &Path, source: StartSource) -> Result<(File, File), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("建立 {}：{error}", dir.display()))?;
    let path = dir.join("record.log");
    let mut out = if source == StartSource::Retry {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| format!("寫不出 {}：{error}", path.display()))?
    } else {
        let _ = std::fs::rename(&path, dir.join("record.log.1"));
        File::create(&path).map_err(|error| format!("寫不出 {}：{error}", path.display()))?
    };
    if source == StartSource::Retry {
        writeln!(
            out,
            "\n--- desktop watchdog retry {} ---",
            sister_core::now_ms()
        )
        .map_err(|error| format!("寫不出 {}：{error}", path.display()))?;
    }
    let err = out
        .try_clone()
        .map_err(|error| format!("寫不出 {}：{error}", path.display()))?;
    Ok((out, err))
}

fn exit_detail(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit {code}"),
        None => "行程被終止，沒有 exit code".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LoginChangedSnapshotDecision, LoginHeartbeatSnapshotError, LoginOccupancyDecision,
        LoginPreflight, LoginRetryDecision, LoginStopPolicy, START_PENDING, SupervisorPhase,
        automatic_effect_blocked_by_human, automatic_login_blocked_by_human,
        cancel_login_preflight, cancel_uncommitted_start, commit_human_explicit_start_if_spawned,
        commit_uncancelled_start, consent_revoke_cancellation_view,
        login_changed_snapshot_decision, login_commit_before_deadline,
        login_heartbeat_snapshot_with, login_observed_owner_blocks_absent,
        login_occupancy_decision, login_retry_decision, login_stop_policy, login_waiting_view,
        reply_to_quit, retained_login_terminal_message, stop_delivery_failure_view,
        stop_delivery_pending_view, take_first_login_intent, with_parent_start_consent,
    };
    use sister_core::control::{StopIntent, StopReason};
    use sister_core::heartbeat::{Phase as BeatPhase, Presence};
    use sister_core::recorder_watchdog as policy;
    use std::cell::Cell;
    use std::sync::{atomic::AtomicU8, mpsc};
    use std::time::{Duration, Instant};

    #[test]
    fn explicit_start_timeout_and_spawn_commit_have_one_winner() {
        let cancelled = AtomicU8::new(START_PENDING);
        assert!(cancel_uncommitted_start(&cancelled));
        assert!(!commit_uncancelled_start(&cancelled));

        let committed = AtomicU8::new(START_PENDING);
        assert!(commit_uncancelled_start(&committed));
        assert!(!cancel_uncommitted_start(&committed));
    }

    #[test]
    fn parent_consent_guard_drops_immediately_after_spawn_returns() {
        struct DropFlag<'a>(&'a Cell<bool>);
        impl Drop for DropFlag<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let dropped = Cell::new(false);
        let result = with_parent_start_consent(DropFlag(&dropped), || {
            assert!(!dropped.get(), "parent guard must cover Command::spawn");
            "spawn returned"
        });
        assert_eq!(result, "spawn returned");
        assert!(
            dropped.get(),
            "parent must not retain consent until a possibly-never-arriving heartbeat"
        );
    }

    #[test]
    fn login_only_clears_a_previous_desktop_quit() {
        assert_eq!(
            login_stop_policy(StopIntent::Absent),
            LoginStopPolicy::Absent
        );
        for intent in [
            StopIntent::Pending(StopReason::DesktopQuit),
            StopIntent::Consumed(StopReason::DesktopQuit),
        ] {
            assert_eq!(login_stop_policy(intent), LoginStopPolicy::DesktopQuit);
        }
        for reason in [StopReason::Requested, StopReason::ConsentRevoked] {
            assert_eq!(
                login_stop_policy(StopIntent::Pending(reason)),
                LoginStopPolicy::Preserve(reason)
            );
            assert_eq!(
                login_stop_policy(StopIntent::Consumed(reason)),
                LoginStopPolicy::Preserve(reason)
            );
        }
        assert_eq!(
            login_stop_policy(StopIntent::Uncheckable),
            LoginStopPolicy::Unknown
        );
    }

    #[test]
    fn absent_never_adopts_lease_or_heartbeat_evidence() {
        assert_eq!(
            login_occupancy_decision(LoginStopPolicy::Absent, true, Presence::NeverStarted),
            LoginOccupancyDecision::Start
        );
        assert_eq!(
            login_occupancy_decision(
                LoginStopPolicy::Absent,
                true,
                Presence::Stopped { at: Some(10) }
            ),
            LoginOccupancyDecision::Start
        );
        assert_eq!(
            login_occupancy_decision(LoginStopPolicy::Absent, false, Presence::NeverStarted),
            LoginOccupancyDecision::External,
            "an occupied lease is an owner even before its first heartbeat"
        );
        for presence in [
            Presence::Live(BeatPhase::Recording),
            Presence::Thinking { at: 10, until: 20 },
            Presence::Stalled {
                at: 10,
                phase: BeatPhase::Recording,
            },
        ] {
            assert_eq!(
                login_occupancy_decision(LoginStopPolicy::Absent, true, presence),
                LoginOccupancyDecision::External,
                "Absent cannot turn fixed or stale unowned evidence into automatic takeover"
            );
        }
    }

    #[test]
    fn desktop_quit_is_the_only_bounded_prior_session_handoff() {
        for presence in [
            Presence::Live(BeatPhase::Recording),
            Presence::Thinking { at: 11, until: 20 },
        ] {
            assert_eq!(
                login_occupancy_decision(LoginStopPolicy::DesktopQuit, false, presence),
                LoginOccupancyDecision::Retry,
                "the old owner may advance from T1 to T2 while it still holds the lease"
            );
            assert_eq!(
                login_occupancy_decision(LoginStopPolicy::DesktopQuit, true, presence),
                LoginOccupancyDecision::Retry,
                "after lease release, live/thinking still has to become stale or a tombstone"
            );
        }
        assert_eq!(
            login_occupancy_decision(
                LoginStopPolicy::DesktopQuit,
                true,
                Presence::Stalled {
                    at: 11,
                    phase: BeatPhase::Recording,
                }
            ),
            LoginOccupancyDecision::Start
        );
        assert_eq!(
            login_occupancy_decision(
                LoginStopPolicy::DesktopQuit,
                true,
                Presence::Stopped { at: Some(12) }
            ),
            LoginOccupancyDecision::Start
        );
    }

    #[test]
    fn absent_external_thinking_then_stopped_never_becomes_a_login_spawn() {
        let external = policy::reduce(policy::State::initial(), policy::Event::ExternalObserved);
        assert!(matches!(external.state, policy::State::External { .. }));
        let stopped = policy::reduce(external.state, policy::Event::ExternalGone);
        assert!(matches!(
            stopped.state,
            policy::State::Stopped {
                reason: policy::StoppedBy::ExternalExit,
                ..
            }
        ));
        assert_eq!(
            login_retry_decision(stopped.state, true),
            LoginRetryDecision::Cancelled,
            "a later tombstone is proof external left, not authority to restart it"
        );
    }

    #[test]
    fn live_to_stopped_cross_read_is_typed_by_the_locked_stop_policy() {
        let changed =
            login_heartbeat_snapshot_with(|| Presence::Live(BeatPhase::Recording), || None);
        assert_eq!(changed, Err(LoginHeartbeatSnapshotError::Changed));
        assert_eq!(
            login_changed_snapshot_decision(Some(LoginStopPolicy::Absent)),
            LoginChangedSnapshotDecision::External
        );
        assert_eq!(
            login_changed_snapshot_decision(Some(LoginStopPolicy::DesktopQuit)),
            LoginChangedSnapshotDecision::Retry
        );
        assert_eq!(
            login_changed_snapshot_decision(None),
            LoginChangedSnapshotDecision::Retry,
            "stop.lock contention has not revealed a typed reason yet"
        );

        let reread = login_heartbeat_snapshot_with(
            || Presence::Stopped { at: Some(12) },
            || panic!("a tombstone must not need a second heartbeat read"),
        );
        assert_eq!(reread, Ok((Presence::Stopped { at: Some(12) }, None)));
    }

    #[test]
    fn login_preflight_is_bounded_and_never_survives_leaving_never_started() {
        let fresh = policy::State::initial();
        assert_eq!(
            login_retry_decision(fresh, false),
            LoginRetryDecision::Expired
        );

        let starting = policy::reduce(fresh, policy::Event::StartRequested).state;
        assert_eq!(
            login_retry_decision(starting, true),
            LoginRetryDecision::Cancelled
        );
        let stopped = policy::reduce(fresh, policy::Event::StopRequested).state;
        assert_eq!(
            login_retry_decision(stopped, true),
            LoginRetryDecision::Cancelled
        );
        let quitting = policy::reduce(fresh, policy::Event::Quit).state;
        assert_eq!(
            login_retry_decision(quitting, true),
            LoginRetryDecision::Cancelled
        );
    }

    #[test]
    fn login_deadline_is_checked_again_at_the_irreversible_commit_point() {
        let now = Instant::now();
        let expires_at = now + Duration::from_secs(2);
        let pending = Some(LoginPreflight {
            heartbeat_at: None,
            observed_owner: false,
            retry_at: now,
            expires_at,
        });
        assert!(login_commit_before_deadline(
            pending,
            expires_at - Duration::from_nanos(1)
        ));
        assert!(
            !login_commit_before_deadline(pending, expires_at),
            "an attempt that started before the deadline cannot clear the marker after it"
        );
        assert!(!login_commit_before_deadline(
            pending,
            expires_at + Duration::from_secs(1)
        ));
    }

    #[test]
    fn owner_seen_while_the_stop_policy_was_unknown_blocks_later_absent() {
        let now = Instant::now();
        let observed = Some(LoginPreflight {
            heartbeat_at: Some(10),
            observed_owner: true,
            retry_at: now,
            expires_at: now + Duration::from_secs(1),
        });
        assert!(login_observed_owner_blocks_absent(
            LoginStopPolicy::Absent,
            observed
        ));
        assert!(!login_observed_owner_blocks_absent(
            LoginStopPolicy::DesktopQuit,
            observed
        ));
    }

    #[test]
    fn login_lock_wait_never_claims_an_owned_recorder_exited() {
        let view =
            login_waiting_view("停止控制 transaction 正忙；這一拍沒有排在鎖後面阻塞".to_owned());
        assert_eq!(view.phase, SupervisorPhase::Uncertain);
        let message = view.message.expect("waiting reason");
        assert!(message.contains("有界期限內完整重試"));
        assert!(!message.contains("owned recorder 已退出"));
        assert!(!message.contains("沒有等待"));
    }

    #[test]
    fn failed_stop_write_cannot_leave_a_due_login_attempt() {
        let now = Instant::now();
        let mut pending = Some(LoginPreflight {
            heartbeat_at: Some(10),
            observed_owner: true,
            retry_at: now - Duration::from_secs(1),
            expires_at: now + Duration::from_secs(1),
        });
        cancel_login_preflight(&mut pending);
        let write: Result<(), &str> = Err("disk full");
        assert!(write.is_err());
        assert!(
            pending.is_none(),
            "failed durable Stop must still cancel Login first"
        );

        let generation = policy::State::initial().generation();
        for effect in [
            policy::Effect::Spawn { generation },
            policy::Effect::ScheduleRetry {
                generation,
                after: policy::RetryDelay::ONE_SECOND,
            },
        ] {
            assert!(automatic_effect_blocked_by_human(true, effect));
            assert!(!automatic_effect_blocked_by_human(false, effect));
        }

        let mut delayed_seen = false;
        assert!(take_first_login_intent(&mut delayed_seen));
        assert!(
            automatic_login_blocked_by_human(true),
            "a first Login delivered after failed Stop must not open a new preflight window"
        );
        assert!(!automatic_login_blocked_by_human(false));
    }

    #[test]
    fn consent_revoke_cancellation_never_leaves_a_fake_watchdog_countdown() {
        let starting =
            policy::reduce(policy::State::initial(), policy::Event::StartRequested).state;
        let generation = starting.generation();
        let backoff = policy::reduce(
            starting,
            policy::Event::SpawnFailed {
                generation,
                at: policy::MonotonicMillis::new(0),
            },
        )
        .state;
        let view = consent_revoke_cancellation_view(backoff, false);
        assert_eq!(view.phase, SupervisorPhase::Uncertain);
        let message = view.message.expect("cancellation truth");
        assert!(message.contains("自動啟動已在記憶體中取消"));
        assert!(message.contains("不宣稱同意書或停止意圖已成功寫入磁碟"));
        assert!(!message.contains("秒後"));
    }

    #[test]
    fn stop_delivery_overlay_has_no_retry_countdown_and_survives_unknown_reads() {
        let backoff = policy::reduce(policy::State::initial(), policy::Event::StartRequested);
        let starting = backoff.state;
        let generation = starting.generation();
        let failed = policy::reduce(
            starting,
            policy::Event::SpawnFailed {
                generation,
                at: policy::MonotonicMillis::new(0),
            },
        );
        assert!(matches!(failed.state, policy::State::Backoff { .. }));

        let pending = stop_delivery_pending_view(failed.state);
        assert_eq!(pending.phase, SupervisorPhase::StopUndelivered);
        let message = pending.message.expect("delivery pending message");
        assert!(!message.contains("秒後"));
        assert!(!message.contains("重試 recorder"));

        let unreadable_after_failure =
            stop_delivery_failure_view(failed.state, true, "stop.lock unreadable");
        assert_eq!(
            unreadable_after_failure.phase,
            SupervisorPhase::StopUndelivered,
            "a later unreadable poll must retain the typed re-send action"
        );
        assert!(
            unreadable_after_failure
                .message
                .as_deref()
                .is_some_and(|message| message.contains("可再按一次停止"))
        );
    }

    #[test]
    fn failed_explicit_start_does_not_override_the_human_stop_latch() {
        let mut cancelled = true;
        let mut delivery_failure = Some("disk full".to_owned());

        let failed: Result<(), &str> = Err("CreateProcess failed");
        assert!(!commit_human_explicit_start_if_spawned(
            &failed,
            &mut cancelled,
            &mut delivery_failure,
        ));
        assert!(cancelled, "Command::spawn failure must preserve the latch");
        assert_eq!(delivery_failure.as_deref(), Some("disk full"));

        let spawned: Result<(), &str> = Ok(());
        assert!(commit_human_explicit_start_if_spawned(
            &spawned,
            &mut cancelled,
            &mut delivery_failure,
        ));
        assert!(!cancelled);
        assert_eq!(delivery_failure, None);
    }

    #[test]
    fn login_intent_is_one_shot_even_after_cancel_or_expiry() {
        let mut seen = false;
        assert!(take_first_login_intent(&mut seen));
        assert!(!take_first_login_intent(&mut seen));
        assert!(!take_first_login_intent(&mut seen));
    }

    #[test]
    fn terminal_login_reason_survives_later_noop_projection_ticks() {
        let state = policy::State::initial();
        let reason = Some("invalid consent");
        assert_eq!(
            retained_login_terminal_message(state, false, reason),
            reason
        );
        assert_eq!(
            retained_login_terminal_message(state, false, reason),
            reason,
            "the next vacant detached-heartbeat tick must not erase the reason"
        );
        let external = policy::reduce(state, policy::Event::ExternalObserved).state;
        assert_eq!(
            retained_login_terminal_message(external, false, reason),
            reason,
            "a detached stale/external projection may change the phase but not erase why Login ended"
        );
        assert_eq!(
            retained_login_terminal_message(state, true, reason),
            None,
            "an explicit higher-priority message still wins"
        );
    }

    #[test]
    fn quit_failure_is_delivered_instead_of_becoming_a_fake_timeout() {
        let (reply, receive) = mpsc::sync_channel(1);
        assert!(!reply_to_quit(reply, Err("durable stop failed".to_owned())));
        assert_eq!(
            receive.recv().expect("quit reply"),
            Err("durable stop failed".to_owned())
        );
    }

    #[test]
    fn only_late_success_requests_worker_side_exit() {
        let (reply, receive) = mpsc::sync_channel(1);
        drop(receive);
        assert!(reply_to_quit(reply, Ok(())));

        let (reply, receive) = mpsc::sync_channel(1);
        drop(receive);
        assert!(!reply_to_quit(reply, Err("durable stop failed".to_owned())));
    }
}
