// 沒有主控台視窗。少了這行，release build 在 Windows 上會多開一個黑框，
// 而她的賣點是「安靜地待在角落」。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 桌面姊妹的外殼。
//!
//! 配方照 PHASES.md Phase 1 寫的：透明、置頂、拖曳條、關閉即收進系統匣。
//! 那份配方來自 TokenMonster（Ted 自己的 repo，MIT），但那邊是 Electron——
//! 這裡是 Tauri，於是有一個結構上的差別值得記下來：
//!
//! Electron 的 renderer 是另一個行程、裡面沒有 Rust，所以 TokenMonster 得在
//! 本機開一個 loopback HTTP gateway、發一次性 bootstrap token、換成 HttpOnly
//! cookie，才能讓畫面跟後端說話。**Tauri 不需要那一整套**，因為後端就是這個
//! 行程裡的 Rust。少一個 port、少一個 token、少一個「有人搶走那個 port 之後
//! 會怎樣」的問題。
//!
//! 這是唯一一個「因為換了殼所以整段不抄」的地方，其餘行為都照舊。

use base64::Engine as _;
use chrono::{Local, Timelike};
use serde::{Deserialize, Serialize};
use sister_core::gatekeeper_candidates::CommitmentRef;
use sister_shell as bounds;
use sister_shell::login_startup::LaunchIntent;
#[cfg(windows)]
use sister_shell::login_startup::launch_intent;
use sister_shell::{PetState, Rect};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, PhysicalPosition, WindowEvent};

mod azure_credential;
mod brain_cli;
mod hands;
mod login_startup;
#[cfg(all(target_os = "macos", feature = "macos-ci-spike"))]
mod macos_ci;
mod master_stop_dispatch;
mod platform_access;
mod recorder_supervisor;
#[cfg(any(windows, test))]
mod single_instance;

use login_startup::{login_startup_read, login_startup_set};
use master_stop_dispatch::{MasterStopAction, master_stop_action_for_menu_id, run_fifo};
use platform_access::{platform_access_open, platform_access_read};
#[cfg(any(windows, test))]
use single_instance::RevealWindow;
#[cfg(windows)]
use single_instance::{ExistingInstanceReveal, reveal_or_defer, take_deferred_reveal};

/// 行動紀錄那一欄一次顯示幾列。
///
/// 有上限就一定要講出被蓋掉了幾列——安靜地截斷會讀成「總共就這幾件事」。
const ACTION_LOG_SHOWN: usize = 20;

const PET: &str = "pet";
const PET_W: i32 = 340;
const PET_H: i32 = 560;
const MASTER_STOP_CHANGED_EVENT: &str = "master-stop-changed";
const MASTER_STOP_FAILED_EVENT: &str = "master-stop-failed";

static SETTINGS_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static ONBOARDING_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static TIMELINE_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static METRICS_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static FRAME_WINDOW_OPENING: AtomicBool = AtomicBool::new(false);
static MASTER_STOP_QUEUE: OnceLock<std::sync::mpsc::Sender<MasterStopJob>> = OnceLock::new();
static NEXT_PRESENTATION_ID: AtomicU64 = AtomicU64::new(1);
static MASTER_STOP_PRESENTATIONS: OnceLock<Mutex<HashMap<u64, PresentationLease>>> =
    OnceLock::new();
#[cfg(windows)]
static SECOND_INSTANCE_REVEAL_PENDING: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static SECOND_INSTANCE_LOGIN_PENDING: AtomicBool = AtomicBool::new(false);

#[cfg(any(windows, test))]
impl<R: tauri::Runtime> RevealWindow for tauri::WebviewWindow<R> {
    fn reveal_show(&self) -> Result<(), String> {
        self.show().map_err(|error| error.to_string())
    }

    fn reveal_focus(&self) -> Result<(), String> {
        self.set_focus().map_err(|error| error.to_string())
    }
}

#[cfg(windows)]
fn single_instance_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_single_instance::init(|app, argv, _cwd| {
        if launch_intent(argv.iter().skip(1)) == LaunchIntent::Login {
            // Windows Run 可能在 primary 已經起來之後才送到。這是一個「把
            // recorder 叫起來」的 intent，不是「把視窗叫到人面前」；兩種
            // pending 分開，才不會登入時突然搶走焦點。
            // 先放 pending 再找 worker，避免 callback 和 setup 正好交錯時把這個
            // intent 掉在地上。兩邊若同時送到，worker 會序列化並拒絕第二份，
            // 最壞是重複問一次，不會重複 spawn。
            SECOND_INSTANCE_LOGIN_PENDING.store(true, Ordering::Release);
            match deliver_login_start(app) {
                Ok(true) => {
                    SECOND_INSTANCE_LOGIN_PENDING.store(false, Ordering::Release);
                    tracing::info!("第二次登入啟動已交給原本的 desktop；不顯示、不聚焦");
                }
                Ok(false) => {
                    tracing::info!("第二次登入啟動早於 recorder supervisor；setup 完成後再交出去")
                }
                Err(error) => {
                    tracing::error!("第二次登入啟動交不出去：{error}");
                }
            }
            return;
        }
        // callback 是在**原本那個** app 裡執行；plugin 會讓後來的行程在 setup、
        // tray 與 recorder ownership 建立前退出。這裡只把原視窗叫回來，絕不
        // 呼叫 `app.exit`、`stop_recording` 或系統匣的 quit handler。
        let window = app.get_webview_window(PET);
        match reveal_or_defer(&SECOND_INSTANCE_REVEAL_PENDING, window.as_ref()) {
            ExistingInstanceReveal::Revealed => {
                tracing::info!("第二次啟動：原本的桌面姊妹已顯示並取得焦點")
            }
            ExistingInstanceReveal::Missing => {
                tracing::info!("第二次啟動早於主視窗建立；setup 完成後再顯示並取得焦點")
            }
            ExistingInstanceReveal::ShowFailed(error) => {
                tracing::error!("第二次啟動：原本的桌面姊妹顯示失敗：{error}")
            }
            ExistingInstanceReveal::FocusFailed(error) => {
                tracing::error!("第二次啟動：原本的桌面姊妹已顯示，但取得焦點失敗：{error}")
            }
        }
    })
}

#[cfg(windows)]
fn deliver_login_start<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<bool, String> {
    let Some(shell) = app.try_state::<Shell>() else {
        return Ok(false);
    };
    let handle = shell.recorder.lock().expect("recorder supervisor").clone();
    let Some(handle) = handle else {
        return Ok(false);
    };
    handle.login_start()?;
    Ok(true)
}

/// 同一扇輔助視窗的非阻塞建立保留。
///
/// `get_webview_window(label)` 和 `WebviewWindowBuilder::build()` 不是同一個
/// 原子操作。tray thread 與 async command 同時連點時，兩邊可能都先看到
/// `None`，再各建一扇同 label 的窗。第二個入口只要知道第一個正在建就夠了；
/// 不等待、不再排一份工作。build 成功、失敗或提早 return 都由 Drop 放回來。
struct WindowOpening(&'static AtomicBool);

impl WindowOpening {
    fn claim(slot: &'static AtomicBool) -> Option<Self> {
        slot.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(slot))
    }
}

impl Drop for WindowOpening {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct Shell {
    state: Mutex<PetState>,
    state_path: PathBuf,
    /// 資料目錄。`None` = 這台機器上問不出使用者的 data dir，那時候暫停鍵
    /// 只能誠實地失敗——一顆按了沒反應、卻假裝有反應的暫停鍵最糟。
    data_dir: Option<PathBuf>,
    /// 資料庫連線。**開得很懶**：她可以在完全沒有資料的機器上開起來，
    /// 使用者按下第一個問題之前不必碰硬碟。
    db: Mutex<Option<sister_core::db::Db>>,
    /// 唯一握著 desktop-owned recorder child 的 worker。放 `Option` 是因為
    /// Tauri state 必須先 manage，worker 要等 setup 拿到 AppHandle 才能建立。
    recorder: Mutex<Option<recorder_supervisor::Handle>>,
    /// Persona omnibus pack 同一時間只准有一個 mutation。Arc 只為了把狀態安全
    /// 帶進 `spawn_blocking`；下載 API 本身不接受 persona、URL 或私人資料。
    asset_operation: Arc<AtomicU8>,
    asset_cancel: Arc<AtomicBool>,
    /// Azure 一次只准一個 native POST。generation 是播放意圖，不冒充能中止
    /// 已經進入 blocking transport 的 socket：取消後晚回應會被丟掉。
    azure_tts_in_flight: Arc<AtomicBool>,
    azure_tts_generation: Arc<AtomicU64>,
    /// `u64::MAX` 表示沒有 request；其餘值是目前 in-flight request 入場時消耗的
    /// baseline generation。每個 request 都先把全域 generation 往前推一代，
    /// 延遲到達的 A cancel 才不會誤殺稍後開始的 B。
    azure_tts_active_generation: Arc<AtomicU64>,
    /// config／credential／desktop consent mutation 與一次 outbound admission 共用。
    /// 誰先拿到鎖，誰就先完成它的線性化點；mutation 在鎖內先 bump 再落地，
    /// speak 則在鎖內重讀並比對 renderer 帶來的完整 gate snapshot。
    azure_tts_admission: Arc<Mutex<()>>,
    /// generation／active request／single-flight 與真正 transport commit 共用的鎖。
    /// mutation/cancel 先拿到就讓 worker 在 POST 前停；worker 先拿到則持有到 blocking
    /// transport 結束，mutation/cancel 可能等待最長 45 秒，但成功回覆後舊 request
    /// 絕不可能才開始 POST。A drop、A cancel 與 B admit 也不能在 atomic 間交錯。
    azure_tts_transition: Arc<Mutex<()>>,
    /// CLI 官方登入與固定 probe 共用一個槽。取消只影響這條使用者發起的工作，
    /// 不會改 config，也不會碰 recorder 正在跑的 brain invocation。
    brain_cli_state: Arc<std::sync::atomic::AtomicU8>,
    /// 對話一次只保留最新題的 CLI invocation。新題會先取消舊題的完整 process
    /// tree；本機檢索不共用這個槽，所以 CLI 不可用時 S1 仍完整回答。
    answer_cli: Mutex<Option<sister_core::brain::Cancellation>>,
    /// renderer 回報的「畫了東西的地方」，視窗座標、CSS 像素。輪詢執行緒拿它
    /// 決定游標底下要不要讓點擊穿透過去。空的代表**還沒收到回報**，那時候
    /// `sister_shell::hit::poll_step` 會當成整片實心、維持可點——見那支函式的說明。
    hit_solid: Mutex<Vec<bounds::Rect>>,
    /// 這一輪畫面回報過的觀測。**只在記憶體裡**——資料目錄裡每多一個檔案，
    /// forget／export／prune 三條刪除路就各要接一次，而且它自己會變成一個新
    /// 的隱私面。代價是重開機之後這本簿子是空的，而報告會把那句話印出來。
    diagnostics: Mutex<sister_core::diagnose::Notebook>,
    /// renderer 說「游標剛動了，現在就去看」。
    ///
    /// 和 `hit_solid` 是兩件事：那個是**算答案的材料**，這個是**該重算了的
    /// 訊號**。renderer 一樣不參與判斷，它只是把輪詢執行緒從睡眠裡叫起來——
    /// 游標進到這扇窗的那一瞬間 webview 收得到 `pointermove`（那時候窗還是
    /// 可點的），比下一次輪詢早最多 `POLL_AWAY_MS` 毫秒。
    hit_wake: std::sync::Arc<bounds::hit::PollGate>,
}

const ASSET_IDLE: u8 = 0;
const ASSET_INSTALLING: u8 = 1;
const ASSET_REMOVING: u8 = 2;
const ASSET_SETTING_VOICE: u8 = 3;
const AZURE_TTS_NO_ACTIVE_GENERATION: u64 = u64::MAX;
// JavaScript Number 能無損表示的最大整數；跨 IPC 的 generation 永遠留在這個範圍。
const AZURE_TTS_MAX_GENERATION: u64 = 9_007_199_254_740_991;

impl Shell {
    fn persist(&self) {
        let snapshot = *self.state.lock().expect("pet state");
        bounds::save(&self.state_path, &snapshot);
    }
}

struct AnswerCliClaim<'a> {
    slot: &'a Mutex<Option<sister_core::brain::Cancellation>>,
    cancellation: sister_core::brain::Cancellation,
}

impl AnswerCliClaim<'_> {
    fn cancellation(&self) -> &sister_core::brain::Cancellation {
        &self.cancellation
    }
}

impl Drop for AnswerCliClaim<'_> {
    fn drop(&mut self) {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|active| active.is_same(&self.cancellation))
        {
            slot.take();
        }
    }
}

fn begin_answer_cli_slot(
    slot: &Mutex<Option<sister_core::brain::Cancellation>>,
) -> AnswerCliClaim<'_> {
    let cancellation = sister_core::brain::Cancellation::default();
    let mut active = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(previous) = active.replace(cancellation.clone()) {
        previous.cancel();
    }
    drop(active);
    AnswerCliClaim { slot, cancellation }
}

fn begin_answer_cli(shell: &Shell) -> AnswerCliClaim<'_> {
    begin_answer_cli_slot(&shell.answer_cli)
}

#[cfg(test)]
mod answer_cli_claim_tests {
    use super::*;

    #[test]
    fn a_new_claim_cancels_the_previous_one_and_old_drop_cannot_clear_new() {
        let slot = Mutex::new(None);
        let first = begin_answer_cli_slot(&slot);
        let first_signal = first.cancellation().clone();
        assert!(!first_signal.is_cancelled());

        let second = begin_answer_cli_slot(&slot);
        let second_signal = second.cancellation().clone();
        assert!(first_signal.is_cancelled());
        assert!(!second_signal.is_cancelled());
        drop(first);
        assert!(
            slot.lock()
                .unwrap()
                .as_ref()
                .is_some_and(|active| active.is_same(&second_signal))
        );

        drop(second);
        assert!(slot.lock().unwrap().is_none());
    }
}

#[tauri::command]
fn answer_cli_cancel(shell: tauri::State<'_, Shell>) -> bool {
    let active = shell
        .answer_cli
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(active) = active {
        active.cancel();
        true
    } else {
        false
    }
}

fn recorder_handle(shell: &Shell) -> Result<recorder_supervisor::Handle, String> {
    shell
        .recorder
        .lock()
        .expect("recorder supervisor")
        .clone()
        .ok_or_else(|| "recorder supervisor 還沒準備好".to_owned())
}

/// 成功後把 handle 從 state 拿掉，讓 `app.exit()` 隨後送出的 ExitRequested
/// 知道 worker 已經完成；失敗則原封不動留下，真人修好原因後還能再試。
fn quit_recorder(shell: &Shell) -> Result<(), String> {
    let handle = shell.recorder.lock().expect("recorder supervisor").clone();
    let Some(handle) = handle else {
        return Ok(());
    };
    handle.quit()?;
    *shell.recorder.lock().expect("recorder supervisor") = None;
    Ok(())
}

/// 一筆答案。這是她說話的全部形狀——**每一筆都帶出處**。
///
/// `frame_id` 是「點開看當時那張畫面」的鑰匙，而**這裡的它已經不只是來源**：
/// 資料庫那一欄只講「這段字抄自哪一幀」，送到畫面上之前會過一次
/// [`sister_core::db::Db::frames_with_image`]，沒有照片的一律變回 `None`。
/// 所以 `Some` 就是點得開，`None` 就是點不開——只記字、截圖節流、額度用完、
/// 過了保留期，通通收在同一個 `None` 底下。
#[derive(Serialize)]
struct Hit {
    /// 題庫要靠它記下「他點開的是哪一筆」（見 `log_click`）。畫面上不顯示。
    chunk_id: i64,
    ts: i64,
    text: String,
    snippet: String,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
    frame_id: Option<i64>,
}

#[derive(Serialize)]
struct GateEvidence {
    frame_id: i64,
    label: String,
}

#[derive(Serialize)]
struct GateDisplay {
    utterance_id: i64,
    form: &'static str,
    text: String,
    evidence: Vec<GateEvidence>,
    suggestion: Option<GateSuggestion>,
}

#[derive(Serialize)]
struct GateSuggestion {
    label: String,
    target_provenance: String,
    /// 畫面按這個 id 回叫，不回叫要執行什麼——見 [`hands_execute`]。
    commitment_id: i64,
}

#[derive(Serialize)]
struct GatekeeperView {
    display: Option<GateDisplay>,
    developer: Option<GatekeeperDeveloper>,
    action_log: Vec<String>,
    /// Native activity guard 留到 renderer 在另一個 IPC round-trip 裡確認要畫這份
    /// view。字串避免 JS `Number` 精度把 u64 lease id 改掉。
    presentation_id: Option<String>,
}

#[derive(Serialize)]
struct GatekeeperDeveloper {
    points_spent: u32,
    points_limit: u32,
    holds: Vec<String>,
}

#[derive(Serialize)]
struct GatekeeperReactionView {
    message: String,
    presentation_id: String,
}

const PRESENTATION_LEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

struct PresentationLease {
    guard: sister_hands::master_stop::ActivityGuard,
    begun: bool,
}

fn presentation_leases() -> &'static Mutex<HashMap<u64, PresentationLease>> {
    MASTER_STOP_PRESENTATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn expire_pending_presentation(id: u64) {
    let mut leases = presentation_leases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if leases.get(&id).is_some_and(|lease| !lease.begun) {
        leases.remove(&id);
    }
}

fn clear_presentations() {
    presentation_leases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// IPC response 回到 renderer 並不代表 JS 已經把它畫完。若外部 CLI 恰好在兩者
/// 之間完成 `stop-all`，晚來的 Promise continuation 會在成功回條之後才顯示新答案。
/// 把 activity guard 暫存在 native，renderer 以 begin → 同步 render → end 完成最後
/// 一小段 commit。尚未 begin 的 response lease 五秒自行回收；begin 之後只由 end、
/// pet window destroy 或 process teardown 釋放，不能讓 timeout 重開 post-success race。
fn hold_presentation(guard: sister_hands::master_stop::ActivityGuard) -> String {
    let id = NEXT_PRESENTATION_ID.fetch_add(1, Ordering::Relaxed);
    presentation_leases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            id,
            PresentationLease {
                guard,
                begun: false,
            },
        );
    std::thread::spawn(move || {
        std::thread::sleep(PRESENTATION_LEASE_TIMEOUT);
        // begin 已回 true 之後不可按時間回收：renderer event loop 若剛好卡住，
        // timeout → stop success → Promise continuation render 會重開同一條競態。
        // Begun 只由 renderer end、window destroy 或 process teardown 釋放。
        expire_pending_presentation(id);
    });
    id.to_string()
}

fn begin_presentation(id: &str) -> Result<bool, String> {
    let id = id
        .parse::<u64>()
        .map_err(|_| "renderer 回傳的 presentation id 讀不懂；沒有顯示晚回覆".to_string())?;
    let mut leases = presentation_leases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let allowed = leases
        .get(&id)
        .and_then(|lease| lease.guard.boundary())
        .is_some();
    if !allowed {
        leases.remove(&id);
    } else if let Some(lease) = leases.get_mut(&id) {
        lease.begun = true;
    }
    Ok(allowed)
}

fn finish_presentation(id: &str) -> Result<(), String> {
    let id = id
        .parse::<u64>()
        .map_err(|_| "renderer 回傳的 presentation id 讀不懂".to_string())?;
    presentation_leases()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
    Ok(())
}

#[tauri::command]
fn master_stop_presentation_begin(presentation_id: String) -> Result<bool, String> {
    begin_presentation(&presentation_id)
}

#[tauri::command]
fn master_stop_presentation_end(presentation_id: String) -> Result<(), String> {
    finish_presentation(&presentation_id)
}

#[derive(Serialize)]
struct MasterStopPresentationView {
    presentation_id: String,
}

/// 本機答案朗讀仍是使用者 trusted click，但它也是 brain answer 的產品輸出。
/// 先由 native admission 鑄一份 lease，renderer begin 後一路留到 utterance
/// end／error／cancel；外部 CLI stop 才能真的等它排乾。
#[tauri::command]
fn answer_local_speech_admit(
    shell: tauri::State<'_, Shell>,
) -> Result<MasterStopPresentationView, String> {
    let guard = admit_desktop_brain(shell.data_dir.as_deref(), "這次本機答案朗讀")?;
    Ok(MasterStopPresentationView {
        presentation_id: hold_presentation(guard),
    })
}

/// 隨程式提供的固定台詞不叫 CLI、不連網；播放仍跨 native master-stop lease，
/// 因此外部 stop-all 會先等正在說的這一句真正停下來。
#[tauri::command]
fn persona_fixed_voice_admit(
    shell: tauri::State<'_, Shell>,
) -> Result<MasterStopPresentationView, String> {
    let guard = admit_desktop_brain(shell.data_dir.as_deref(), "這次 Persona 本機固定語音")?;
    Ok(MasterStopPresentationView {
        presentation_id: hold_presentation(guard),
    })
}

fn admit_desktop_brain(
    data_dir: Option<&Path>,
    work: &str,
) -> Result<sister_hands::master_stop::ActivityGuard, String> {
    let data_dir = data_dir.ok_or_else(|| format!("找不到資料目錄，{work}沒有開始"))?;
    sister_hands::master_stop::admit(data_dir).ok_or_else(
        || match sister_hands::master_stop::state(data_dir) {
            sister_hands::master_stop::State::Stopping => {
                format!("正在完成全停，{work}沒有開始；排乾完成後可從系統匣解除全停")
            }
            sister_hands::master_stop::State::Stopped => {
                format!("現在是三層全停，{work}沒有開始；要繼續請從系統匣解除全停")
            }
            sister_hands::master_stop::State::Uncertain => format!(
                "全停協定目前讀不到可靠狀態，為安全起見{work}沒有開始；可從系統匣嘗試解除全停"
            ),
            sister_hands::master_stop::State::Clear => {
                format!("全停 admission 取得失敗，{work}沒有開始")
            }
        },
    )
}

/// action log 的「兩種零」在還看得到檔案的這一層分開。`Replay::default()` 同時
/// 代表檔案不存在與存在但為空；等 replay 完才問就已經永遠答不回來了。
fn action_log_lines(data_dir: Option<&Path>) -> Result<Vec<String>, String> {
    let Some(dir) = data_dir else {
        return Ok(vec!["這台機器上找不到資料目錄，action log 讀不到。".into()]);
    };
    let log = sister_hands::ActionLog::in_data_dir(dir);
    let file_exists = log
        .path()
        .try_exists()
        .map_err(|e| format!("檢查 action log 失敗：{e}"))?;
    let replay = log
        .replay()
        .map_err(|e| format!("讀 action log 失敗：{e:#}"))?;
    let lines = hands::recent_replay_lines(&replay, ACTION_LOG_SHOWN);
    if lines.is_empty() {
        Ok(vec![
            sister_hands::replay_copy::empty_run_log_message(file_exists).into(),
        ])
    } else {
        Ok(lines)
    }
}

/// 把 core 的守門員判決接到每天真的會用的字母人。
///
/// **這個 command 是被輪詢的**，所以它的每一步都要問「同一件事被問第二次的
/// 時候會怎樣」：
///
/// 1. 同一件事今天只記一次帳（[`Db::utterance_today_for`]）。少了這一條，
///    「每天 5 點」量到的不是她講了幾句話，是畫面重新整理了幾次——五秒鐘
///    就用完，而人一句都還沒看到。
/// 2. 今天已經開口、人還沒回應的那一句**繼續顯示，不重扣點數**。她已經講
///    過了，重扣等於同一句話收兩次錢。
/// 3. 判決分兩趟：先全部判完、排名，**然後才寫**。落選的那幾句記成
///    [`HoldReason::OutrankedThisRound`]，不是記成 `spoke`——她一次只講一句，
///    而落選的那幾句人從來沒看到過。
/// 4. 擋下的理由**變了**才記一列。同一個理由連續輪詢不重複寫。
#[tauri::command(async)]
fn gatekeeper_check(shell: tauri::State<'_, Shell>) -> Result<GatekeeperView, String> {
    // Gatekeeper 會寫每次判決與可能的 spoke 列，所以不是一份無害的 status poll。
    // guard 從候選讀取一路留到 view 交給 presentation lease；全停成功回條之後，
    // 這一輪不會才新增產品帳或第一次畫出一張主動卡。
    let master_stop_admission = admit_desktop_brain(shell.data_dir.as_deref(), "守門員這一輪")?;
    let now = sister_core::now_ms();
    let day_key = sister_core::local_day::local_day_key(now)
        .ok_or_else(|| "現在時間無法換成本地日期".to_string())?;
    let config = sister_core::Config::load(&config_path()?).map_err(|e| format!("{e:#}"))?;
    let local = Local::now();
    let quiet_hours_end = config
        .gatekeeper
        .quiet_end_at((local.hour() * 60 + local.minute()) as u16)
        .map_err(|e| format!("{e:#}"))?;
    let presence = shell
        .data_dir
        .as_ref()
        .map(|d| sister_core::heartbeat::presence(d, now))
        .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
    let mut view = with_db_mut(&shell, |db| {
        let candidates =
            sister_core::gatekeeper_candidates::collect(db, now).map_err(|e| format!("{e:#}"))?;
        let first = db
            .first_recording_at()
            .map_err(|e| format!("{e:#}"))?
            .unwrap_or(now);
        let days_since = u32::try_from(now.saturating_sub(first) / 86_400_000).unwrap_or(u32::MAX);
        use sister_core::db::UtteranceDecision;
        use sister_core::gatekeeper::{HoldReason, Verdict};

        // 一次快照。迴圈裡重讀的話，第一句開口會把第二句的預算算成已經花掉，
        // 而這一輪最後只會有一句真的講出去。
        let spent = db
            .points_spent_today(&day_key)
            .map_err(|e| format!("{e:#}"))?;
        let ever = db.has_ever_spoken().map_err(|e| format!("{e:#}"))?;

        // ── 第一趟：判，但不寫。 ──────────────────────────────────
        // 今天已經開口、人還沒回應的那一句，繼續掛著（不重判、不重扣）。
        let mut pending: Option<sister_core::db::UtteranceRow> = None;
        let mut judged: Vec<(sister_core::gatekeeper::Candidate, Verdict, Option<String>)> =
            Vec::new();
        let mut holds = Vec::new();
        for candidate in candidates {
            let existing = db
                .utterance_today_for(&day_key, candidate.category, &candidate.evidence)
                .map_err(|e| format!("{e:#}"))?;
            match &existing {
                Some(row) if matches!(row.decision, UtteranceDecision::Spoke { .. }) => {
                    // 已經講過了。人還沒回應就繼續顯示；回應過就今天不再提。
                    if row.reaction.is_none() && pending.as_ref().is_none_or(|p| p.id < row.id) {
                        pending = existing;
                    }
                    continue;
                }
                _ => {}
            }
            let previous_hold = existing.and_then(|row| match row.decision {
                UtteranceDecision::Held { reason } => Some(reason),
                UtteranceDecision::Spoke { .. } => None,
            });
            let cooldown_remaining_minutes = db
                .last_spoke_at(candidate.category)
                .map_err(|e| format!("{e:#}"))?
                .and_then(|last| {
                    let elapsed = now.saturating_sub(last) / 60_000;
                    (elapsed < i64::from(config.gatekeeper.cooldown_minutes))
                        .then(|| config.gatekeeper.cooldown_minutes - elapsed as u32)
                });
            let verdict = sister_core::gatekeeper::decide(&sister_core::gatekeeper::GateInput {
                candidate: candidate.clone(),
                presence,
                quiet_hours_end: quiet_hours_end.clone(),
                // 桌面目前也沒有可靠的跨平台前景視窗幾何；沒量到不是 Windowed。
                focus_mode: sister_core::gatekeeper::FocusMode::Unmeasured,
                days_since_first_recording: days_since,
                cold_start_days: config.gatekeeper.cold_start_days,
                has_ever_spoken: ever,
                cooldown_remaining_minutes,
                points_spent_today: spent,
                daily_budget_points: config.gatekeeper.daily_budget_points,
                min_score: config.gatekeeper.min_score,
            });
            judged.push((candidate, verdict, previous_hold));
        }

        // ── 排名：她一次只講一句。 ────────────────────────────────
        // 形式高的優先（建議卡 > 一行字 > 微光），同形式比分數。
        let mut winner = None;
        for (i, (candidate, verdict, _)) in judged.iter().enumerate() {
            if let Verdict::Speak { form, .. } = verdict {
                let rank = (form.cost(), candidate.score());
                if winner.is_none_or(|(_, best): (usize, (u32, f64))| {
                    rank.0 > best.0 || (rank.0 == best.0 && rank.1 > best.1)
                }) {
                    winner = Some((i, rank));
                }
            }
        }
        let winner = winner.map(|(i, _)| i);
        // 落選那幾句要講得出「輸給了哪一句」，所以先把贏家的原文留下來。
        let winner_text = winner.map(|i| judged[i].0.text.clone()).unwrap_or_default();

        // ── 第二趟：寫。 ─────────────────────────────────────────
        let mut display = None;
        for (i, (candidate, verdict, previous_hold)) in judged.into_iter().enumerate() {
            // 過得了關但不是這一輪最該講的那一句——記成落選，不是記成講過了。
            let verdict = match (&verdict, winner) {
                (Verdict::Speak { .. }, Some(w)) if w != i => {
                    Verdict::Hold(HoldReason::OutrankedThisRound {
                        by_text: winner_text.clone(),
                    })
                }
                _ => verdict,
            };
            // 擋下的理由沒變就不重複記——輪詢一次寫一列的話，這張表數的是
            // 畫面重新整理的次數，不是她考慮過幾次。
            let same_as_before = match (&verdict, &previous_hold) {
                (Verdict::Hold(reason), Some(before)) => {
                    before.starts_with(&format!("{}: ", reason.code()))
                }
                _ => false,
            };
            if same_as_before {
                if let Verdict::Hold(reason) = &verdict {
                    holds.push(reason.message());
                }
                continue;
            }
            let id = db
                .record_utterance(&sister_core::db::UtteranceInsert {
                    ts: now,
                    day_key: &day_key,
                    candidate: &candidate,
                    verdict: &verdict,
                })
                .map_err(|e| format!("{e:#}"))?;
            match verdict {
                Verdict::Speak { form, cost: _ } => {
                    let reference = CommitmentRef::from_candidate(candidate.commitment_id);
                    let suggestion = gate_suggestion(db, &reference, &mut holds)?;
                    let chips = frame_chips(&candidate.evidence);
                    holds.extend(evidence_not_on_screen(&candidate.evidence, &chips));
                    display = Some(GateDisplay {
                        utterance_id: id,
                        form: form.as_str(),
                        text: candidate.text,
                        evidence: chips,
                        suggestion,
                    });
                }
                Verdict::Hold(reason) => holds.push(reason.message()),
            }
        }
        // 舊帳優先：她已經講過而人還沒回應的那一句還掛在畫面上。
        if let Some(row) = pending {
            let form = match &row.decision {
                UtteranceDecision::Spoke { form, .. } => form.as_str(),
                // 上面已經濾過只留 Spoke；走到這裡代表濾網壞了，要看得見。
                UtteranceDecision::Held { .. } => {
                    return Err("pending 裡混進了一列 held——濾網壞了".into());
                }
            };
            let reference = CommitmentRef::from_evidence(&row.evidence);
            let suggestion = gate_suggestion(db, &reference, &mut holds)?;
            let chips = frame_chips(&row.evidence);
            holds.extend(evidence_not_on_screen(&row.evidence, &chips));
            display = Some(GateDisplay {
                utterance_id: row.id,
                form,
                text: row.text,
                evidence: chips,
                suggestion,
            });
        }
        let developer = if config.shell.developer_mode {
            Some(GatekeeperDeveloper {
                points_spent: db
                    .points_spent_today(&day_key)
                    .map_err(|e| format!("{e:#}"))?,
                points_limit: config.gatekeeper.daily_budget_points,
                holds: holds.into_iter().rev().take(5).collect(),
            })
        } else {
            None
        };
        let action_log = action_log_lines(shell.data_dir.as_deref())?;
        Ok(GatekeeperView {
            display,
            developer,
            action_log,
            presentation_id: None,
        })
    })?;
    view.presentation_id = Some(hold_presentation(master_stop_admission));
    Ok(view)
}

#[cfg(test)]
mod action_log_view_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-desktop-action-log-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp action-log dir");
        dir
    }

    #[test]
    fn missing_and_existing_but_empty_logs_are_two_different_histories() {
        let dir = temp_dir("two-zeros");
        let missing = action_log_lines(Some(&dir)).expect("missing log is readable");
        std::fs::write(dir.join("action-log.jsonl"), "").expect("make empty log");
        let emptied = action_log_lines(Some(&dir)).expect("empty log is readable");
        assert_ne!(missing, emptied);
        assert!(missing.join("\n").contains("從來沒有"), "{missing:?}");
        assert!(emptied.join("\n").contains("列被刪光"), "{emptied:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 這張卡上要不要放「要我幫你…嗎」那顆按鈕。
///
/// 每一種不放按鈕的理由都各自留一句話給開發者模式看。安靜地回 `None` 的只有
/// 一種：這張卡根本不是在講承諾，那本來就沒有什麼下一步可做。
fn gate_suggestion(
    db: &sister_core::db::Db,
    reference: &CommitmentRef,
    developer_lines: &mut Vec<String>,
) -> Result<Option<GateSuggestion>, String> {
    use sister_hands::commitment_action::{AllowedNextStep, parse_allowed_next_step};
    if let Some(why) = reference.why_no_button() {
        developer_lines.push(why);
    }
    let CommitmentRef::One(id) = reference else {
        return Ok(None);
    };
    let id = *id;
    let row = db
        .commitment_by_id(id)
        .map_err(|e| format!("讀承諾 #{id} 失敗：{e:#}"))?;
    // 承諾可以被 `forget` 的血緣 cascade 整列刪掉，而那句話還掛在畫面上。
    // 這不該讓整個 gatekeeper 面板變成一句錯誤訊息、把她要講的話一起吃掉。
    let Some(row) = row else {
        developer_lines.push(format!("這句話指向的承諾 #{id} 已經不在了，所以不放按鈕。"));
        return Ok(None);
    };
    match parse_allowed_next_step(row.allowed_next_step.as_deref()) {
        AllowedNextStep::Missing => Ok(None),
        AllowedNextStep::Unparseable { raw, reason } => {
            developer_lines.push(format!("承諾 #{id} 的下一步讀不懂：{reason}；原文：{raw}"));
            Ok(None)
        }
        AllowedNextStep::Suggestion(button) => {
            let target = sister_core::db::target_app_for_button(
                &button,
                row.allowed_next_step_fact,
                |fact_id, raw| {
                    db.app_for_target_fact(fact_id, raw)
                        .map_err(|e| format!("讀下一步目標 fact #{fact_id} 失敗：{e:#}"))
                },
            )?;
            Ok(Some(gate_suggestion_from_target(
                id,
                &button,
                target.as_ref(),
            )))
        }
    }
}

fn gate_suggestion_from_target(
    commitment_id: i64,
    button: &sister_hands::SuggestionButton,
    target: Option<&sister_core::db::TargetApp>,
) -> GateSuggestion {
    let text = sister_core::db::suggestion_text(button, target);
    GateSuggestion {
        label: text.label,
        target_provenance: text.target_provenance,
        commitment_id,
    }
}

/// 按下那顆按鈕。
///
/// **參數是承諾的 id，不是要執行的動作。** 畫面送回來的字不會被拿去執行——
/// 要做什麼由這裡重新去資料庫讀一次。差別在於：前者是「畫面說要開這個」，
/// 後者是「她自己提過要開這個」，而 SPEC §9.7 要的是後者。
#[tauri::command(async)]
fn hands_execute(commitment_id: i64, shell: tauri::State<'_, Shell>) -> Result<String, String> {
    let data_dir = shell
        .data_dir
        .as_deref()
        .ok_or_else(|| "這台機器上找不到資料目錄，沒有動手。".to_string())?;
    let raw = with_db(&shell, |db| {
        let row = db
            .commitment_by_id(commitment_id)
            .map_err(|e| format!("讀承諾 #{commitment_id} 失敗：{e:#}"))?
            .ok_or_else(|| format!("承諾 #{commitment_id} 已經不在了，沒有動手。"))?;
        row.allowed_next_step
            .ok_or_else(|| format!("承諾 #{commitment_id} 上沒有寫下一步，沒有動手。"))
    })?;
    hands::execute_logged(data_dir, &raw, sister_core::now_ms())
}

/// evidence ref 裡指得到畫面的那幾個，變成可以點開的 chip。
///
/// 認不出來的 ref（`commitment:`、`segment:`、`fact:`）**不進來**：那不是
/// 「這張沒有畫面」，是「這一種 ref 指的不是畫面」。時間軸那邊的
/// `guessRow` 是同一套做法（`button.see` → `open_frame`）。
fn frame_chips(evidence: &[String]) -> Vec<GateEvidence> {
    evidence
        .iter()
        .filter_map(|r| {
            let frame_id = r.strip_prefix("frame:")?.parse::<i64>().ok()?;
            Some(GateEvidence {
                frame_id,
                label: format!("畫面 #{frame_id}"),
            })
        })
        .collect()
}

/// 這句話的出處一顆都點不開的時候，開發者那一欄要講出出處是什麼。
///
/// 畫面上收起那條空的 chip 帶不會說謊，但也不會說話。日終那種卡的出處是
/// `reviewer_run:` 和 `daysummary:`——「這句話沒有出處」和「這句話的出處
/// 不是畫面」是兩件事，而收起來之後兩者長得一樣。
fn evidence_not_on_screen(evidence: &[String], chips: &[GateEvidence]) -> Option<String> {
    if !chips.is_empty() || evidence.is_empty() {
        return None;
    }
    Some(format!(
        "這句話的出處點不開：{}——這幾種 ref 指的不是畫面。",
        evidence.join("、")
    ))
}

#[tauri::command(async)]
fn gatekeeper_react(
    utterance_id: i64,
    close: bool,
    shell: tauri::State<'_, Shell>,
) -> Result<GatekeeperReactionView, String> {
    let master_stop_admission = admit_desktop_brain(shell.data_dir.as_deref(), "這次守門員回應")?;
    let message = with_db_mut(&shell, |db| {
        let reaction = if close {
            sister_core::gatekeeper::Reaction::Close
        } else {
            sister_core::gatekeeper::Reaction::Other
        };
        let effect =
            sister_core::gatekeeper::react(db, utterance_id, reaction, sister_core::now_ms())
                .map_err(|e| format!("{e:#}"))?;
        Ok(match effect {
            sister_core::gatekeeper::CommitmentReaction::MarkDead { .. } => "這張記憶不會再提了",
            sister_core::gatekeeper::CommitmentReaction::SnoozeAndLowerWeight => {
                "先收起來，之後再說"
            }
            sister_core::gatekeeper::CommitmentReaction::None => {
                "收到你的回饋；這一則沒有可結案或延後的承諾"
            }
        }
        .to_string())
    })?;
    Ok(GatekeeperReactionView {
        message,
        presentation_id: hold_presentation(master_stop_admission),
    })
}

#[tauri::command]
fn toggle_pin(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> bool {
    let pinned = {
        let mut state = shell.state.lock().expect("pet state");
        state.pinned = !state.pinned;
        state.pinned
    };
    if let Some(win) = app.get_webview_window(PET) {
        let _ = win.set_always_on_top(pinned);
    }
    shell.persist();
    pinned
}

/// renderer 回報：現在畫面上哪幾塊是實心的。
///
/// 只有 renderer 知道這件事——泡泡冒出來、她換角色、輸入列長高，都會改。所以
/// 真相從那邊推過來，Rust 這邊只存著最後一次的答案。
///
/// 座標是視窗座標的 CSS 像素，和 `getBoundingClientRect()` 同一套；輪詢那邊會
/// 拿 `scale_factor()` 把實體游標座標換算成同一套再比。
#[tauri::command]
fn pet_solid_set(solid: Vec<SolidRect>, shell: tauri::State<'_, Shell>) {
    let rects = solid
        .into_iter()
        .map(|r| bounds::Rect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        })
        .collect();
    *shell.hit_solid.lock().expect("hit solid") = rects;
}

/// renderer 回報：游標在這扇窗裡動了。
///
/// 不帶座標，也不帶答案。判斷整套仍然留在輪詢執行緒裡——它會自己去問
/// `cursor_position()`，那是唯一權威的來源。這條指令唯一的作用是讓那一拍
/// **現在**就發生，而不是等最多 `hit::POLL_AWAY_MS` 毫秒。
///
/// 為什麼不讓 renderer 自己翻開關：那會變成第二個記著「上一次翻到哪一邊」的
/// 人，其中一邊翻完另一邊不知道，開關就會停在那裡不再送出。而且同步 command
/// 跑在主執行緒上，輪詢執行緒的 `set_ignore_cursor_events` 要 dispatch 回主
/// 執行緒——兩邊搶同一把鎖會死鎖。
///
/// 只有「可點」的時候 renderer 才收得到 `pointermove`，所以這條線只加速
/// 「可點 → 穿透」那一個方向。反方向（已經穿透中、游標移到她身上）webview
/// 是瞎的，仍然只有輪詢救得回來，節奏是 `hit::POLL_NEAR_MS`。
#[tauri::command]
fn pet_pointer_moved(shell: tauri::State<'_, Shell>) {
    bounds::hit::nudge(&shell.hit_wake);
}

/// IPC 上的長方形。`sister_shell::Rect` 自己不 derive `Deserialize`——它是給
/// 螢幕幾何用的內部型別，不該因為這條線就變成對 renderer 開放的輸入格式。
#[derive(serde::Deserialize)]
struct SolidRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// 她現在有沒有在看。
///
/// 每次都去讀磁碟，**不快取在這個行程裡**：按下暫停的可能是系統匣、終端機，
/// 也可能是上一次開機留下的狀態。這個視窗只是一面鏡子，真相在 data dir 的
/// transaction state 裡。
#[tauri::command]
fn pause_state(shell: tauri::State<'_, Shell>) -> bool {
    match &shell.data_dir {
        Some(dir) => sister_core::pause::is_paused(dir),
        // 問不出資料目錄 = 不知道她在做什麼。按照 `pause` 模組的規則，
        // 不確定就報暫停——寧可顯示得比實際保守。
        None => true,
    }
}

/// 現在到底有沒有人在錄。
///
/// 這和暫停是**兩個不同的問題**，而字母人以前只問得出後者：暫停鍵沒被按下
/// 的時候它就顯示「在聽」——即使根本沒有人把 `sister record` 跑起來。那是這個
/// 產品唯一不能說的那種謊：使用者照著那三個字相信她記得住今天，然後某天問
/// 「剛剛發生什麼事」，得到一片空白。
///
/// 判斷靠 recorder 每 5 秒蓋一次的時戳（見 [`sister_core::heartbeat`]），
/// 不靠 `sessions.ended_at`——那一列在 recorder 當掉的時候永遠停在 NULL。
///
/// # 為什麼回五個字串
///
/// 上一版回 `is_recording` 那個布林，而**它把「正在起來」歸進「沒有人在
/// 錄」**——於是 `app.js` 那個等她起來的迴圈（`startRecording`）在一顆一年份
/// 的資料庫上一定逾時：`Db::open` 要跑好幾分鐘，那 25 秒裡 `is_recording` 從
/// 頭到尾是 false，畫面說「等了 25 秒還沒有心跳」。**而心跳從第一秒就在**
/// （`ops::BootBeat` 在開資料庫之前先蓋一次），那段註解自己也是這樣寫的。
/// 同一顆按鈕在系統匣上更難看：那邊看到 false 就去 `start_recording`，而那一
/// 支的第一道閘門是 `is_occupied`——按下去只會回一句「已經有一個 sister
/// record 在跑了」。
///
/// 「她在錄嗎」和「有人佔著這個目錄嗎」是兩個問題（`heartbeat.rs` 開頭那段就
/// 是在講這件事），而一個布林只答得出一個。收工想最後一段是第四種：沒在錄，
/// 但行程還在——那兩分鐘裡回 `"none"` 會讓他按開始，然後兩句話對打。
#[tauri::command]
fn recording_state(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> String {
    let presence = shell
        .data_dir
        .as_ref()
        .map(|dir| sister_core::heartbeat::presence(dir, sister_core::now_ms()))
        .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
    // `watching_word` 保留 `unreadable`：前者是沒量到，`none` 才是量到沒有
    // 活的 recorder。Renderer 在 Backoff/GaveUp 只有真的 `none` 才能提供 Retry。
    let now = sister_core::heartbeat::watching_word(presence);
    // 順手把系統匣那兩顆的字改對——手上已經有答案了，不必再讀一次磁碟。這不是
    // 唯一的刷新時機（見 [`refresh_tray`]），是最即時的那一個：視窗開著的時候，
    // 選單和字母人講的是同一秒的事。
    //
    // 那兩行字問的是「按下去會發生什麼」，不是「她在錄嗎」——正在起來的那一個
    // 也停得掉，也會被「結束」帶走，所以兩種都算佔著。
    set_record_labels(&app, presence);
    now.to_string()
}

/// 系統匣那三行字：全部重新去問一次磁碟。
///
/// 三個項目講的都是「她現在在幹嘛」，而三個都會過期：
///
/// * 「開始記錄／停止記錄」和「結束（記錄也會停）」以前只在 [`recording_state`]
///   被呼叫時才刷，而那是**畫面**每 5 秒問一次的——`visibilitychange` 一關就
///   停（見 `app.js` 的 `updatePollGate`）。也就是說**視窗一收進系統匣，系統匣
///   的字就凍住了**，而那正是它變成唯一介面的那一刻。收起來之後 recorder 被
///   Ctrl+C 掉，選單還寫著「結束（記錄也會停）」——他讀到的是「她在錄」。
/// * 「暫停記錄／繼續記錄」過期得更早：它只在 [`announce_pause`] 裡改，而那
///   只有**這個行程自己**按下去才會走到。終端機裡一句 `sister pause` 之後，
///   選單永遠寫著「暫停記錄」，按下去等於把她放出來。
///
/// 所以刷新得由這一端自己排（見 `main` 裡那條 [`TRAY_REFRESH`]），不是搭畫面
/// 的便車——搭便車的東西會在那個人走掉的時候一起停。
fn refresh_tray(app: &tauri::AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    let (presence, paused) = match &shell.data_dir {
        Some(dir) => (
            // `is_occupied` 不是 `is_recording`：這兩行字問的是「按下去會發生
            // 什麼」。理由在 [`recording_state`] 上面。
            sister_core::heartbeat::presence(dir, sister_core::now_ms()),
            sister_core::pause::is_paused(dir),
        ),
        // 問不出資料目錄的時候，和 `recording_state` / `pause_state` 倒向同一邊：
        // 不確定就往「她做得比較少」那邊講。
        None => (sister_core::heartbeat::Presence::NeverStarted, true),
    };
    set_record_labels(app, presence);
    if let Some(item) = app.try_state::<PauseItem>() {
        let _ = item.0.set_text(pause_label(paused));
    }
    let (stop, resume) = shell
        .data_dir
        .as_ref()
        .map(|dir| sister_hands::kill_switch::tray_hands_labels(dir))
        .unwrap_or_else(sister_hands::kill_switch::tray_hands_unknown_labels);
    if let Some(item) = app.try_state::<HandsStopItem>() {
        let _ = item.0.set_text(stop);
    }
    if let Some(item) = app.try_state::<HandsResumeItem>() {
        let _ = item.0.set_text(resume);
    }
    let phase = master_stop_phase(shell.data_dir.as_deref());
    let (stop, resume) = master_stop_labels(phase);
    if let Some(item) = app.try_state::<MasterStopItem>() {
        let _ = item.0.set_text(stop);
    }
    if let Some(item) = app.try_state::<MasterResumeItem>() {
        let _ = item.0.set_text(resume);
    }
    if let Some(phase) = phase {
        let _ = app.emit(MASTER_STOP_CHANGED_EVENT, phase);
    }
}

fn master_stop_phase(data_dir: Option<&Path>) -> Option<sister_hands::master_stop::State> {
    data_dir.map(sister_hands::master_stop::state)
}

/// 系統匣是兩個固定方向的動作，不是 toggle。這樣 stale label 最多只會重做同一
/// 個冪等動作，絕不會把使用者按下的「全部停止」重新解讀成「解除全停」。
fn master_stop_labels(
    phase: Option<sister_hands::master_stop::State>,
) -> (&'static str, &'static str) {
    use sister_hands::master_stop::State;
    match phase {
        Some(State::Stopped) => ("全部停止（現在是全停）", "解除全停"),
        Some(State::Stopping) => ("全部停止（正在完成）", "解除全停"),
        Some(State::Clear) => ("全部停止", "解除全停（現在沒有全停）"),
        Some(State::Uncertain) => ("全部停止（狀態讀不到）", "解除全停（嘗試重設）"),
        None => ("全部停止（找不到資料目錄）", "解除全停（找不到資料目錄）"),
    }
}

#[tauri::command]
fn master_stop_state(
    shell: tauri::State<'_, Shell>,
) -> Result<sister_hands::master_stop::State, String> {
    master_stop_phase(shell.data_dir.as_deref())
        .ok_or_else(|| "找不到資料目錄，現在不能確認全停狀態".to_owned())
}

fn set_master_stop_with_observer<F>(
    data_dir: Option<&Path>,
    action: MasterStopAction,
    pending_observer: F,
) -> Result<(), String>
where
    F: FnOnce(),
{
    let dir = data_dir.ok_or_else(|| "找不到資料目錄，全停開關沒有作用".to_owned())?;
    match action {
        MasterStopAction::Engage => sister_hands::master_stop::engage_with_pending_observer(
            dir,
            sister_core::now_ms(),
            pending_observer,
        )
        .map_err(|error| format!("全部停止失敗：{error}")),
        MasterStopAction::Release => sister_hands::master_stop::release(dir)
            .map_err(|error| format!("解除全停失敗：{error}")),
    }
}

#[cfg(test)]
fn set_master_stop(data_dir: Option<&Path>, action: MasterStopAction) -> Result<(), String> {
    set_master_stop_with_observer(data_dir, action, || {})
}

fn announce_master_stop_state(app: &tauri::AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    if let Some(phase) = master_stop_phase(shell.data_dir.as_deref()) {
        let _ = app.emit(MASTER_STOP_CHANGED_EVENT, phase);
    }
}

fn finish_master_stop_menu(app: &tauri::AppHandle, result: Result<(), String>) {
    if let Err(error) = result {
        tracing::error!("全停開關切換失敗：{error}");
        if let Some(win) = app.get_webview_window(PET) {
            let _ = win.show();
            let _ = win.set_focus();
        }
        let _ = app.emit(MASTER_STOP_FAILED_EVENT, error);
    }
    refresh_tray(app);
}

struct MasterStopJob {
    app: tauri::AppHandle,
    data_dir: Option<PathBuf>,
    action: MasterStopAction,
}

fn run_master_stop_job(job: MasterStopJob) {
    let stopping_app = job.app.clone();
    let result = set_master_stop_with_observer(job.data_dir.as_deref(), job.action, move || {
        let on_main = stopping_app.clone();
        if stopping_app
            .run_on_main_thread(move || refresh_tray(&on_main))
            .is_err()
        {
            tracing::error!("全停已開始排乾，但 tray 主迴圈已經結束，無法顯示 Stopping");
        }
    });
    let on_main = job.app.clone();
    if job
        .app
        .run_on_main_thread(move || finish_master_stop_menu(&on_main, result))
        .is_err()
    {
        tracing::error!("全停 worker 已收尾，但 tray 主迴圈已經結束，無法顯示結果");
    }
}

fn master_stop_queue() -> &'static std::sync::mpsc::Sender<MasterStopJob> {
    MASTER_STOP_QUEUE.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || run_fifo(receiver, run_master_stop_job));
        sender
    })
}

/// Production tray callback 的唯一全停出口。方向只由 menu id policy 決定。真正的
/// drain 進背景 worker；不然一份 120 秒才收尾的 brain 工作會把 tray 主迴圈整段凍住，
/// 使用者也永遠看不到已經安全發佈的 `Stopping`。
fn dispatch_master_stop_menu(app: &tauri::AppHandle, menu_id: &str) {
    let Some(action) = master_stop_action_for_menu_id(menu_id) else {
        return;
    };
    let data_dir = app.state::<Shell>().data_dir.clone();
    if master_stop_queue()
        .send(MasterStopJob {
            app: app.clone(),
            data_dir,
            action,
        })
        .is_err()
    {
        finish_master_stop_menu(
            app,
            Err("全停 worker 已經結束，這次按鍵沒有執行".to_string()),
        );
    }
}

#[cfg(test)]
mod master_stop_desktop_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-desktop-master-stop-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create master-stop test dir");
        dir
    }

    #[test]
    fn labels_distinguish_all_four_master_stop_states() {
        use sister_hands::master_stop::State;
        assert_eq!(
            master_stop_labels(Some(State::Clear)),
            ("全部停止", "解除全停（現在沒有全停）")
        );
        assert_eq!(
            master_stop_labels(Some(State::Stopping)),
            ("全部停止（正在完成）", "解除全停")
        );
        assert_eq!(
            master_stop_labels(Some(State::Stopped)),
            ("全部停止（現在是全停）", "解除全停")
        );
        assert_eq!(
            master_stop_labels(Some(State::Uncertain)),
            ("全部停止（狀態讀不到）", "解除全停（嘗試重設）")
        );
    }

    #[test]
    fn native_observation_does_not_call_pending_or_broken_protocol_stopped() {
        use sister_hands::master_stop::State;
        let dir = temp_dir("pending-and-uncertain");
        std::fs::write(dir.join("master.stop.pending"), b"1000").unwrap();
        assert_eq!(master_stop_phase(Some(&dir)), Some(State::Uncertain));
        std::fs::remove_file(dir.join("master.stop.pending")).unwrap();
        std::fs::remove_file(dir.join(sister_hands::master_stop::ACTIVITY_LOCK)).unwrap();
        std::fs::create_dir_all(dir.join(sister_hands::master_stop::ACTIVITY_LOCK)).unwrap();
        assert_eq!(master_stop_phase(Some(&dir)), Some(State::Uncertain));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn release_action_preserves_pause_and_hands_switches() {
        let dir = temp_dir("independent-switches");
        sister_core::pause::set_paused(&dir, true, 1).expect("pause");
        sister_hands::kill_switch::pull(&dir, 2).expect("pull hands");

        set_master_stop(Some(&dir), MasterStopAction::Engage).expect("engage master stop");
        assert_eq!(
            master_stop_phase(Some(&dir)),
            Some(sister_hands::master_stop::State::Stopped)
        );
        set_master_stop(Some(&dir), MasterStopAction::Release).expect("release master stop");

        assert!(sister_core::pause::is_paused(&dir));
        assert!(sister_hands::kill_switch::is_pulled(&dir));
        assert_eq!(
            master_stop_phase(Some(&dir)),
            Some(sister_hands::master_stop::State::Clear)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fifo_worker_keeps_a_queued_release_after_a_blocked_engage() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = temp_dir("fifo-engage-release");
        let active = sister_hands::master_stop::admit(&dir).expect("hold old activity");
        let (job_tx, job_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            run_fifo(job_rx, |action| {
                let result = set_master_stop(Some(&worker_dir), action);
                done_tx.send((action, result)).unwrap();
            });
        });

        job_tx.send(MasterStopAction::Engage).unwrap();
        for _ in 0..500 {
            if sister_hands::master_stop::state(&dir) == sister_hands::master_stop::State::Stopping
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            sister_hands::master_stop::state(&dir),
            sister_hands::master_stop::State::Stopping
        );
        job_tx.send(MasterStopAction::Release).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(40)).is_err());
        drop(active);

        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (MasterStopAction::Engage, Ok(()))
        );
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (MasterStopAction::Release, Ok(()))
        );
        drop(job_tx);
        worker.join().unwrap();
        assert_eq!(
            sister_hands::master_stop::state(&dir),
            sister_hands::master_stop::State::Clear,
            "second click must be the final state"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn begun_renderer_presentation_keeps_stop_from_finishing_until_end() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = temp_dir("renderer-presentation");
        let guard = sister_hands::master_stop::admit(&dir).expect("admit answer");
        let presentation_id = hold_presentation(guard);
        assert!(begin_presentation(&presentation_id).unwrap());

        let (done_tx, done_rx) = mpsc::channel();
        let stop_dir = dir.clone();
        let stop = std::thread::spawn(move || {
            done_tx
                .send(sister_hands::master_stop::engage(&stop_dir, 9))
                .unwrap();
        });
        for _ in 0..500 {
            if sister_hands::master_stop::state(&dir) == sister_hands::master_stop::State::Stopping
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            sister_hands::master_stop::state(&dir),
            sister_hands::master_stop::State::Stopping
        );
        assert!(
            done_rx.recv_timeout(Duration::from_millis(40)).is_err(),
            "stop succeeded before renderer ended the begun presentation"
        );

        // Pending timeout 只准回收尚未 begin 的 lease；直接呼叫同一支 expiry seam
        // 模擬五秒 timer，begun guard 仍必須活著。
        expire_pending_presentation(presentation_id.parse().unwrap());
        assert!(done_rx.recv_timeout(Duration::from_millis(40)).is_err());
        finish_presentation(&presentation_id).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        stop.join().unwrap();
        assert_eq!(
            sister_hands::master_stop::state(&dir),
            sister_hands::master_stop::State::Stopped
        );
        sister_hands::master_stop::release(&dir).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 「開始／停止記錄」和「結束」那兩行字。分出來是因為 [`recording_state`] 手上
/// 已經有答案了，不必為了改字再讀一次磁碟。
///
/// 收完整 Presence，因為「佔著」有兩種按鍵後果：錄製／開機中能停止，Thinking
/// 只能等收尾。標籤直接沿用 core 裡 exhaustive 的三向答案。
fn set_record_labels(app: &tauri::AppHandle, presence: sister_core::heartbeat::Presence) {
    let phase = app
        .try_state::<Shell>()
        .and_then(|shell| recorder_handle(shell.inner()).ok())
        .map(|handle| handle.view().phase);
    if let Some(item) = app.try_state::<RecordItem>() {
        item.show(record_menu_presentation(phase, presence));
    }
    if let Some(item) = app.try_state::<QuitItem>() {
        let _ = item.0.set_text(quit_menu_label(phase, presence));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordMenuAction {
    Start,
    Stop,
    Wait,
}

/// 系統匣真正顯示的字，和點下那行字時唯一允許的動作。
///
/// 兩個欄位必須由同一次 evidence snapshot 一起生出來；click handler 不可以再讀
/// 一次磁碟另算 action。否則畫面仍寫「停止記錄」的五秒內，record 若剛好收工，
/// 那一點會被重新解讀成 Start——使用者按停止，產品反而開始錄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordMenuPresentation {
    action: RecordMenuAction,
    label: &'static str,
}

/// 心跳仍是外部 recorder 的真相；supervisor 只覆蓋自己能證明的 child／retry。
/// 未送達的 Stop 與正在等外部墓碑是更精確的 supervisor 證據；其餘狀態遇到
/// Thinking 時，才由「已停止擷取、只等最後收尾」優先。
fn record_menu_presentation(
    phase: Option<recorder_supervisor::SupervisorPhase>,
    presence: sister_core::heartbeat::Presence,
) -> RecordMenuPresentation {
    if phase == Some(recorder_supervisor::SupervisorPhase::StopUndelivered) {
        return RecordMenuPresentation {
            action: RecordMenuAction::Stop,
            label: "再送一次停止要求",
        };
    }
    if phase == Some(recorder_supervisor::SupervisorPhase::StoppingExternal) {
        return RecordMenuPresentation {
            action: RecordMenuAction::Wait,
            label: "正在等外部 recorder 收工",
        };
    }
    if matches!(presence, sister_core::heartbeat::Presence::Thinking { .. }) {
        return RecordMenuPresentation {
            action: RecordMenuAction::Wait,
            label: sister_core::heartbeat::tray_record_label(presence),
        };
    }
    match phase {
        Some(
            recorder_supervisor::SupervisorPhase::Starting
            | recorder_supervisor::SupervisorPhase::Running,
        ) => RecordMenuPresentation {
            action: RecordMenuAction::Stop,
            label: "停止記錄",
        },
        Some(recorder_supervisor::SupervisorPhase::Cooling) => RecordMenuPresentation {
            action: RecordMenuAction::Wait,
            label: "正在確認 recorder 已收工",
        },
        Some(recorder_supervisor::SupervisorPhase::StoppingExternal) => {
            unreachable!("handled before heartbeat precedence")
        }
        Some(
            recorder_supervisor::SupervisorPhase::Backoff
            | recorder_supervisor::SupervisorPhase::GaveUp,
        ) if matches!(
            presence,
            sister_core::heartbeat::Presence::NeverStarted
                | sister_core::heartbeat::Presence::Stopped { .. }
                | sister_core::heartbeat::Presence::Stalled { .. }
        ) =>
        {
            RecordMenuPresentation {
                action: RecordMenuAction::Start,
                label: if phase == Some(recorder_supervisor::SupervisorPhase::Backoff) {
                    "現在重試"
                } else {
                    "再試一次"
                },
            }
        }
        Some(
            recorder_supervisor::SupervisorPhase::Backoff
            | recorder_supervisor::SupervisorPhase::GaveUp,
        ) => RecordMenuPresentation {
            action: RecordMenuAction::Wait,
            label: "記錄狀態不明",
        },
        Some(recorder_supervisor::SupervisorPhase::External) => match presence {
            sister_core::heartbeat::Presence::Live(_) => RecordMenuPresentation {
                action: RecordMenuAction::Stop,
                label: sister_core::heartbeat::tray_record_label(presence),
            },
            sister_core::heartbeat::Presence::NeverStarted
            | sister_core::heartbeat::Presence::Stopped { .. }
            | sister_core::heartbeat::Presence::Stalled { .. }
            | sister_core::heartbeat::Presence::Unreadable => RecordMenuPresentation {
                action: RecordMenuAction::Wait,
                label: "正在確認外部 recorder 已收工",
            },
            sister_core::heartbeat::Presence::Thinking { .. } => {
                unreachable!("thinking presence is handled before supervisor phase")
            }
        },
        Some(
            recorder_supervisor::SupervisorPhase::Uncertain
            | recorder_supervisor::SupervisorPhase::Quitting,
        ) => RecordMenuPresentation {
            action: RecordMenuAction::Wait,
            label: if phase == Some(recorder_supervisor::SupervisorPhase::Quitting) {
                "正在結束…"
            } else {
                "記錄狀態不明"
            },
        },
        Some(recorder_supervisor::SupervisorPhase::StopUndelivered) => {
            unreachable!("handled before heartbeat precedence")
        }
        Some(recorder_supervisor::SupervisorPhase::Stopped) | None => {
            let action = match sister_core::heartbeat::tray_record_action(presence) {
                sister_core::heartbeat::TrayRecordAction::Start => RecordMenuAction::Start,
                sister_core::heartbeat::TrayRecordAction::Stop => RecordMenuAction::Stop,
                sister_core::heartbeat::TrayRecordAction::WaitForThinking => RecordMenuAction::Wait,
                sister_core::heartbeat::TrayRecordAction::WaitForUnknown => RecordMenuAction::Wait,
            };
            RecordMenuPresentation {
                action,
                label: sister_core::heartbeat::tray_record_label(presence),
            }
        }
    }
}

fn quit_menu_label(
    phase: Option<recorder_supervisor::SupervisorPhase>,
    presence: sister_core::heartbeat::Presence,
) -> &'static str {
    match phase {
        Some(
            recorder_supervisor::SupervisorPhase::Starting
            | recorder_supervisor::SupervisorPhase::Running,
        ) => "結束（記錄也會停）",
        Some(recorder_supervisor::SupervisorPhase::Cooling) => "結束",
        Some(recorder_supervisor::SupervisorPhase::StoppingExternal) => "結束（正在等記錄停止）",
        Some(recorder_supervisor::SupervisorPhase::Backoff) => "結束（自動重試也會停）",
        Some(recorder_supervisor::SupervisorPhase::Quitting) => "正在結束…",
        Some(recorder_supervisor::SupervisorPhase::StopUndelivered) => "結束（停止要求尚未送達）",
        Some(
            recorder_supervisor::SupervisorPhase::Stopped
            | recorder_supervisor::SupervisorPhase::GaveUp
            | recorder_supervisor::SupervisorPhase::External
            | recorder_supervisor::SupervisorPhase::Uncertain,
        )
        | None => sister_core::heartbeat::tray_quit_label(presence),
    }
}

#[cfg(test)]
mod recorder_menu_tests {
    use super::*;
    use recorder_supervisor::SupervisorPhase as Phase;
    use sister_core::heartbeat::{Phase as BeatPhase, Presence};

    #[test]
    fn supervisor_failures_only_offer_retry_after_vacancy_is_proven() {
        for presence in [
            Presence::NeverStarted,
            Presence::Stopped { at: Some(1) },
            Presence::Stalled {
                at: 1,
                phase: BeatPhase::Recording,
            },
        ] {
            let backoff = record_menu_presentation(Some(Phase::Backoff), presence);
            assert_eq!(backoff.action, RecordMenuAction::Start);
            assert_eq!(backoff.label, "現在重試");

            let gave_up = record_menu_presentation(Some(Phase::GaveUp), presence);
            assert_eq!(gave_up.action, RecordMenuAction::Start);
            assert_eq!(gave_up.label, "再試一次");
        }

        for presence in [
            Presence::Live(BeatPhase::Booting),
            Presence::Live(BeatPhase::Recording),
            Presence::Unreadable,
        ] {
            assert_eq!(
                record_menu_presentation(Some(Phase::Backoff), presence).action,
                RecordMenuAction::Wait
            );
            assert_eq!(
                record_menu_presentation(Some(Phase::GaveUp), presence).action,
                RecordMenuAction::Wait
            );
        }
    }

    #[test]
    fn owned_child_and_unknown_probe_never_offer_a_second_recorder() {
        assert_eq!(
            record_menu_presentation(Some(Phase::Starting), Presence::NeverStarted).action,
            RecordMenuAction::Stop
        );
        assert_eq!(
            record_menu_presentation(Some(Phase::Uncertain), Presence::NeverStarted).action,
            RecordMenuAction::Wait
        );

        for phase in [Some(Phase::Stopped), None] {
            let shown = record_menu_presentation(phase, Presence::Unreadable);
            assert_eq!(shown.action, RecordMenuAction::Wait);
            assert_eq!(shown.label, "記錄狀態不明");
        }
    }

    #[test]
    fn undelivered_stop_always_remains_a_stop_action() {
        for presence in [
            Presence::NeverStarted,
            Presence::Unreadable,
            Presence::Live(BeatPhase::Booting),
            Presence::Live(BeatPhase::Recording),
            Presence::Thinking { at: 1, until: 2 },
            Presence::Stopped { at: Some(1) },
            Presence::Stalled {
                at: 1,
                phase: BeatPhase::Recording,
            },
        ] {
            let shown = record_menu_presentation(Some(Phase::StopUndelivered), presence);
            assert_eq!(shown.action, RecordMenuAction::Stop);
            assert_eq!(shown.label, "再送一次停止要求");
        }
    }

    #[test]
    fn cooling_never_offers_an_action_while_the_old_heartbeat_expires() {
        for presence in [
            Presence::NeverStarted,
            Presence::Unreadable,
            Presence::Live(BeatPhase::Booting),
            Presence::Live(BeatPhase::Recording),
            Presence::Thinking { at: 1, until: 2 },
            Presence::Stopped { at: Some(1) },
            Presence::Stalled {
                at: 1,
                phase: BeatPhase::Recording,
            },
        ] {
            assert_eq!(
                record_menu_presentation(Some(Phase::Cooling), presence).action,
                RecordMenuAction::Wait
            );
        }
    }

    #[test]
    fn external_recorder_only_delegates_a_live_or_thinking_heartbeat() {
        for presence in [
            Presence::Live(BeatPhase::Booting),
            Presence::Live(BeatPhase::Recording),
        ] {
            assert_eq!(
                record_menu_presentation(Some(Phase::External), presence).action,
                RecordMenuAction::Stop
            );
        }

        assert_eq!(
            record_menu_presentation(
                Some(Phase::External),
                Presence::Thinking { at: 1, until: 2 }
            )
            .action,
            RecordMenuAction::Wait
        );

        for presence in [
            Presence::NeverStarted,
            Presence::Unreadable,
            Presence::Stopped { at: Some(1) },
            Presence::Stalled {
                at: 1,
                phase: BeatPhase::Recording,
            },
        ] {
            assert_eq!(
                record_menu_presentation(Some(Phase::External), presence).action,
                RecordMenuAction::Wait
            );
        }
    }

    #[test]
    fn stopping_external_never_offers_an_action_before_its_tombstone() {
        for presence in [
            Presence::NeverStarted,
            Presence::Unreadable,
            Presence::Live(BeatPhase::Booting),
            Presence::Live(BeatPhase::Recording),
            Presence::Thinking { at: 1, until: 2 },
            Presence::Stopped { at: Some(1) },
            Presence::Stalled {
                at: 1,
                phase: BeatPhase::Recording,
            },
        ] {
            let shown = record_menu_presentation(Some(Phase::StoppingExternal), presence);
            assert_eq!(shown.action, RecordMenuAction::Wait);
            assert_eq!(shown.label, "正在等外部 recorder 收工");
        }
    }

    #[test]
    fn new_supervisor_phases_keep_the_quit_label_truthful() {
        assert_eq!(
            quit_menu_label(Some(Phase::Cooling), Presence::Live(BeatPhase::Recording)),
            "結束"
        );
        assert_eq!(
            quit_menu_label(
                Some(Phase::StoppingExternal),
                Presence::Live(BeatPhase::Recording)
            ),
            "結束（正在等記錄停止）"
        );

        for presence in [
            Presence::NeverStarted,
            Presence::Unreadable,
            Presence::Live(BeatPhase::Booting),
            Presence::Live(BeatPhase::Recording),
            Presence::Thinking { at: 1, until: 2 },
            Presence::Stopped { at: Some(1) },
            Presence::Stalled {
                at: 1,
                phase: BeatPhase::Recording,
            },
        ] {
            assert_eq!(
                quit_menu_label(Some(Phase::External), presence),
                sister_core::heartbeat::tray_quit_label(presence)
            );
        }
    }

    #[test]
    fn a_rendered_stop_action_never_turns_into_start_when_the_disk_changes() {
        let rendered =
            record_menu_presentation(Some(Phase::Running), Presence::Live(BeatPhase::Recording));
        assert_eq!(rendered.label, "停止記錄");
        assert_eq!(rendered.action, RecordMenuAction::Stop);

        // Click 使用 `RecordItem::action()` 保存的這份 presentation；不拿這個較晚
        // 的墓碑重算。反向的 stale Start 則仍會撞 heartbeat/OS lease 而 fail closed。
        let later = Presence::Stopped { at: Some(2) };
        assert_eq!(
            record_menu_presentation(Some(Phase::Stopped), later).action,
            RecordMenuAction::Start
        );
        assert_eq!(rendered.action, RecordMenuAction::Stop);
    }
}

/// 系統匣刷字的節奏。
///
/// 跟著心跳走（`heartbeat::BEAT_EVERY_MS` 是 5 秒）。更慢的話，「她剛剛當掉了」
/// 和「選單還在說她活著」之間會有一段看得到的空窗。成本是每 5 秒兩個小檔案的
/// 讀取——和畫面開著時本來就在做的事一樣，只是現在收起來也做。
const TRAY_REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

/// 上一場錄製是什麼時候、為什麼結束的。
///
/// 「沒有人在記錄」永遠帶著下一個問題：那她是什麼時候停的、為什麼停的。
/// 答不出來的話，那句灰字讀起來就像故障——而「你自己按了停止」和「同意書
/// 被撤回」的下一步完全不同。
///
/// 只在她沒在錄的時候問（見畫面那一邊）：正在錄的時候，最後一場就是這一場，
/// 而它還沒有結束。
///
/// 時戳直接給出去，不在這裡排版：時區是畫面那一邊的東西，而它已經有一個
/// `when()` 在用同一種格式印出處了（時間軸那條 `tz_offset_ms` 是同一條紀律）。
#[tauri::command(async)]
fn last_recording_end(shell: tauri::State<'_, Shell>) -> Option<LastRun> {
    // 資料庫還沒有／打不開的時候回 `None`。那句灰字自己站得住，這裡不該為了
    // 補一行字而把「她還沒錄過任何東西」講成一個錯誤。
    //
    // `None` 現在還有第三種來源：他把整段時間忘掉了，那幾場的紀錄跟著走了
    // （`retention::delete_empty_sessions`）。畫面那邊照樣只是少講一句
    // 「上一次幾點停的」——少講不等於講錯，而「她錄過」那件事有
    // [`has_ever_recorded`] 專門在答，時間軸就是拿它畫那一頁的。
    with_db(&shell, |db| {
        Ok(db
            .last_session()
            .map_err(|e| format!("{e:#}"))?
            .map(|s| LastRun {
                started_at: s.started_at,
                ended_at: s.ended_at,
                why: s
                    .reason
                    .as_deref()
                    .map(|r| sister_core::model::EndReason::describe(r).to_string()),
                // 沒有理由**而且**那一場的事件全被清掉了：理由本來很可能寫過
                // ，是後來被保留期或 `sister forget` 帶走的。畫面上要講成
                // 「查不出來了」，不是「那時候還沒在記」——見
                // `LastSession::events_left`。
                why_gone: s.reason.is_none() && s.events_left == 0,
            }))
    })
    .ok()
    .flatten()
}

/// 她**曾經**開始記過東西嗎。
///
/// 和 [`last_recording_end`] 分開，因為那一支答的是「上一場長什麼樣」，而
/// 一場錄製的紀錄現在會跟著它記下來的東西一起消失（見
/// `retention::delete_empty_sessions`）。時間軸以前拿它當「她錄過嗎」用，
/// 於是一個把整顆資料庫忘光的人會拿到「她還沒記得任何東西。跑 sister record
/// 之後再回來看。」——叫他重做一件他剛剛才故意做掉的事。
///
/// 反過來也不能拿它當「她錄過嗎」：最後一場**當掉**的話，那一列撐得過
/// `forget`（那道守衛不准碰還沒收尾的最新一列，因為那可能是此刻正在錄的那一
/// 場），於是同一顆被清空的資料庫會因為「上一場有沒有當掉」給出兩種答案。
///
/// 這一支問的是 `meta` 裡那個位元：沒有時間、沒有長度，忘不掉也重建不出東西。
#[tauri::command(async)]
fn has_ever_recorded(shell: tauri::State<'_, Shell>) -> bool {
    with_db(&shell, |db| {
        db.ever_recorded().map_err(|e| format!("{e:#}"))
    })
    .unwrap_or(false)
}

/// 她有沒有**真的存下來過一列內容**。
///
/// 上面那一支答不出這一題，而時間軸那一頁需要的正是這一題：它拿
/// `has_ever_recorded` 當「這些紀錄是被忘掉的」的根據，可是那個旗標在
/// `start_session` 就翻成 true，**第一張畫面之前**。於是一台
/// `capture.enabled = false` 的機器——她跑完、一個字都沒記到、`sister forget`
/// 從來沒有被執行過——會在那一頁上讀到「這些紀錄是被忘掉的，或是過了保留
/// 期」。那是指控一件沒發生的事。
///
/// 兩支分開而不是合成一個結構回去：`has_ever_recorded` 已經有呼叫端和假後端
/// 在用，而這兩個位元各自都有單獨成立的意思。見
/// [`sister_core::db::Db::ever_stored`]。
#[tauri::command(async)]
fn has_ever_stored(shell: tauri::State<'_, Shell>) -> bool {
    with_db(&shell, |db| db.ever_stored().map_err(|e| format!("{e:#}"))).unwrap_or(false)
}

/// 上一場錄製（見 [`last_recording_end`]）。
#[derive(Serialize)]
struct LastRun {
    started_at: i64,
    /// `None` = 沒有好好結束。問這支命令的前提是「現在沒有心跳」，所以
    /// `Db::last_session` 上那個「還是它正在跑」的歧義在這裡已經被排除了。
    ended_at: Option<i64>,
    /// 已經翻成人話的理由。`None` 有兩種意思，靠 [`why_gone`](Self::why_gone) 分。
    why: Option<String>,
    /// `why` 是 `None` 的原因是**紀錄被清掉了**，不是那一版沒在記。
    why_gone: bool,
}

/// 系統匣裡的那一顆開始／停止。理由和 [`PauseItem`] 一樣：一個永遠寫著同一句
/// 話的切換項目，會讓人按出他沒想要的那個方向。
struct RecordItem {
    item: MenuItem<tauri::Wry>,
    shown: Mutex<RecordMenuPresentation>,
}

impl RecordItem {
    /// Menu mutation 和 click 都在 Tauri 主迴圈上；仍先 disable，讓未來若呼叫端
    /// 換執行緒，也不會在 label/action 交棒中間留下可按的反向操作。
    fn show(&self, next: RecordMenuPresentation) {
        if self.item.set_enabled(false).is_err() {
            return;
        }
        if self.item.set_text(next.label).is_ok() {
            *self.shown.lock().expect("record menu presentation") = next;
        }
        // re-enable 失敗只會留下按不到的安全退化，不會讓 action 與 label 對調。
        let _ = self.item.set_enabled(true);
    }

    fn action(&self) -> RecordMenuAction {
        self.shown.lock().expect("record menu presentation").action
    }
}
struct HandsStopItem(MenuItem<tauri::Wry>);
struct HandsResumeItem(MenuItem<tauri::Wry>);
struct MasterStopItem(MenuItem<tauri::Wry>);
struct MasterResumeItem(MenuItem<tauri::Wry>);

/// 系統匣裡的「結束」。存起來的理由見 [`quit_label`]。
struct QuitItem(MenuItem<tauri::Wry>);

/// 把 recorder start intent 交給唯一的 supervisor worker。worker 才能碰 Child，
/// 所以按鈕、系統匣、登入啟動與 retry 不會同時各開一份。
#[tauri::command(async)]
fn start_recording(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    recorder_handle(shell.inner())?.explicit_start()
}

/// 請 recorder 收工。
///
/// 寫一個檔案，不去 kill 那個行程——理由寫在 [`sister_core::control`]：
/// `TerminateProcess` 會讓她死在半路，留下一筆永遠不會結束的 session
/// 和一個還在說「我在錄」的心跳檔。
#[tauri::command]
fn stop_recording(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    recorder_handle(shell.inner())?.stop()
}

#[tauri::command]
fn recorder_supervisor_state(
    shell: tauri::State<'_, Shell>,
) -> recorder_supervisor::SupervisorView {
    recorder_handle(shell.inner())
        .map_or_else(recorder_supervisor::SupervisorView::unavailable, |handle| {
            handle.view()
        })
}

/// recorder 最後說的那幾句話。
///
/// 按了「開始記錄」卻沒有起來的時候，理由已經寫在 `record.log` 裡了——但那個
/// 檔案在 `%APPDATA%` 深處，而正在看著一個沒反應的按鈕的人不會去翻它。把最後
/// 幾行直接端到畫面上，「按了沒反應」才會變成一句看得懂的話。
/// `record.log` 是**按下去的那一刻**才建的，而建之前 [`start_log_at`] 會把上
/// 一輪改名成 `.1`。所以「按了沒起來、再按一次」的第二下，唯一寫著原因的那一份
/// 已經變成 `.1`，新開的那一份是空的——以前這裡只讀新的那一份，於是畫面說
/// 「沒有留下任何理由」，而理由就躺在它旁邊。
#[tauri::command(async)]
fn recorder_log_tail(shell: tauri::State<'_, Shell>) -> String {
    const LINES: usize = 6;
    let Some(dir) = shell.data_dir.as_ref() else {
        return String::new();
    };
    let tail = |name: &str| -> String {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            return String::new();
        };
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .rev()
            .take(LINES)
            .collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    };
    let now = tail("record.log");
    if !now.is_empty() {
        return now;
    }
    match tail("record.log.1").as_str() {
        "" => String::new(),
        // 講明白這是哪一輪的。不講的話，兩輪前的一句錯誤會被讀成「她剛剛就是
        // 這樣死的」——而那兩件事要做的處置不一樣。
        before => format!("（這一輪還沒寫出東西，以下是上一輪的 record.log）\n{before}"),
    }
}

#[tauri::command]
fn toggle_pause(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) -> Result<bool, String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，暫停鍵沒有作用".to_string())?;
    // 讀取與翻轉必須在同一把跨行程鎖裡：熱鍵和系統匣若同時先各自讀一次，
    // 兩次 toggle 會從同一個舊值算出同一個答案，實際只翻一次。
    let next = sister_core::pause::toggle_paused(dir, sister_core::now_ms())
        .map_err(|e| format!("{e:#}"))?;
    announce_pause(&app, next);
    Ok(next)
}

/// 把新的暫停狀態同時送到視窗和系統匣。
///
/// 兩個地方都要更新，因為兩個地方都能觸發它——只更新自己那一邊的話，
/// 從系統匣暫停之後，視窗裡的字母人會繼續一臉「我在聽」。
fn announce_pause(app: &tauri::AppHandle, paused: bool) {
    // 解除 pause 只改 paused.flag。若全停仍在，先把全停真相送回 renderer，
    // 再送 pause 的新值，畫面就不會在兩個 event 中間短暫冒出「在聽」。
    announce_master_stop_state(app);
    let _ = app.emit("pause-changed", paused);
    if let Some(item) = app.try_state::<PauseItem>() {
        let _ = item.0.set_text(pause_label(paused));
    }
}

fn pause_label(paused: bool) -> &'static str {
    if paused {
        "繼續記錄"
    } else {
        "暫停記錄"
    }
}

/// 系統匣裡的那一顆暫停。存起來是為了改它的字——選單上一個永遠寫著
/// 「暫停記錄」的項目，在已經暫停的時候會讓人再按一次，然後把它打開。
struct PauseItem(MenuItem<tauri::Wry>);

#[tauri::command]
fn hide_to_tray(app: tauri::AppHandle, shell: tauri::State<'_, Shell>) {
    if let Some(win) = app.get_webview_window(PET) {
        remember_position(&win, &shell);
        let _ = win.hide();
    }
    shell.persist();
}

/// 借用那顆資料庫，需要的話當場開。
///
/// 開得很懶（她可以在完全沒有資料的機器上啟動），但**每一支要讀資料的命令都
/// 得走這裡**。在這之前只有 `ask` 會開：於是先點開時間軸、還沒問過任何問題的
/// 那條路上，看圖那支會回「資料庫還沒開」——一句只有寫程式的人看得懂、而且
/// 根本不是真正原因的錯誤訊息。多一個入口就多一條這種路。
///
/// **叫得動這裡的命令一律要標 `#[tauri::command(async)]`。** Tauri 的同步命令
/// 跑在主執行緒上，而這一支的第一次呼叫可能要幾分鐘：`Db::open` 會把還沒跑過
/// 的 migration 跑完，其中 003 要把整張 `text_chunks` 讀出來重算 bigram。主
/// 執行緒卡住的時候整個殼都跟著卡——暫停鍵、系統匣、連拖曳都沒反應，而畫面
/// 停在「想一下…」上，看起來就只是她想很久。這是一個真的發生過的當機畫面，
/// 不是理論。
fn with_db<T>(
    shell: &tauri::State<'_, Shell>,
    f: impl FnOnce(&sister_core::db::Db) -> Result<T, String>,
) -> Result<T, String> {
    let mut slot = shell.db.lock().map_err(|_| "資料庫鎖壞了".to_string())?;
    if slot.is_none() {
        let dir = sister_core::config::Config::default_data_dir()
            .ok_or_else(|| "找不到資料目錄".to_string())?;
        let path = sister_core::config::Config::db_path(&dir);
        // 她還沒錄過任何東西的時候，這裡**不要**建一個空資料庫然後假裝正常。
        // 「我還沒有任何記憶」跟「我查不到」是兩件不同的事，使用者該分得出來。
        if !path.exists() {
            return Err("還沒有任何記憶——先跑 `sister record`".to_string());
        }
        // `{e:#}` 而不是 `to_string()`：anyhow 的 `to_string()` 只給最外面那
        // 一層 context，這裡就是「open C:\…\sister.db」——一句廢話。真正寫給
        // 他看的那幾行（例如「這份資料庫比這個執行檔新」）躺在底下。這一頁
        // 上 26 個 `map_err` 全部同一個理由。
        *slot = Some(sister_core::db::Db::open(&path).map_err(|e| format!("{e:#}"))?);
    }
    f(slot.as_ref().expect("just opened"))
}

/// 同上，但拿得到可變借用。刪東西的那兩支要用這個。
fn with_db_mut<T>(
    shell: &tauri::State<'_, Shell>,
    f: impl FnOnce(&mut sister_core::db::Db) -> Result<T, String>,
) -> Result<T, String> {
    with_db(shell, |_| Ok(()))?;
    let mut slot = shell.db.lock().map_err(|_| "資料庫鎖壞了".to_string())?;
    f(slot.as_mut().expect("with_db opened it"))
}

/// 一次回答。
///
/// `kind` 不是給程式判斷用的，是**要講給他聽的**：他打了「剛剛發生什麼事」，
/// 而她回的東西跟那七個字一個都不像——不說清楚「我把它當成時間問題了」，看起來
/// 就只是她答非所問。
///
/// 判斷放在 core（[`sister_core::question`]），不在這裡也不在畫面上。`sister
/// query` 和這一頁必須對同一句話給同一種答案，各抄一份遲早會變成兩種行為。
#[derive(Serialize)]
struct Answer {
    /// Native guard 不能在 IPC response serialize 完就放掉；renderer 先 begin、同步
    /// 畫完，再 end。外部 CLI 的 stop-all 才不會在 Promise continuation 前先回成功。
    presentation_id: Option<String>,
    kind: &'static str,
    followup: Option<String>,
    closure_notice: Option<String>,
    /// 她拿去比對的那串字，**但只在它是黏出來的時候**。`None` = 沒什麼好講。
    ///
    /// `question::terms` 會把「剛剛」「那個」剝掉，剝到不足兩個字還會往回退
    /// 一格——而那一格常常退進虛字裡：「剛剛那個板」→「個板」、「剛剛看到的
    /// 人」→「的人」。於是兩種完全不同的處境印出同一句「我記得的東西裡沒有
    /// 這件事」：他打的字真的沒出現過，跟她根本沒找他打的字。有命中的那一半
    /// 更難看出來——「的人」在一年份的螢幕文字裡什麼都比得到，於是他拿到一串
    /// 毫不相干的東西，而唯一讀得出來的意思是「這東西壞了」。
    ///
    /// 前者他無能為力；後者他只要把那個詞重打一次就好。唯一能讓他分辨的，是
    /// 看到她到底拿什麼去比對。
    ///
    /// 和 `sister query --json` 的 `terms` 是同一件事的兩種送法，**不要合成
    /// 一個**：那一份給機器讀（也是 Phase 2 評測語料的來源），所以每一題都要
    /// 有；這一份給人看，每次都報一句只會讓人學會忽略它，所以剝對了就閉嘴，
    /// 只有黏出不是詞的東西才出聲。判斷交給 `terms_with_retreat`——它答的是
    /// 「有沒有退過邊界」，而退邊界是唯一一種黏得出非詞的來源。
    searched: Option<String>,
    /// 這一題在題庫裡的編號。點開出處的時候要掛回來（見 `log_click`）。
    ///
    /// `None` = 沒記成功。畫面那一邊要能在沒有編號的情況下照常運作——記不成
    /// 題庫不該讓一個能回答的問題變成錯誤。
    query_id: Option<i64>,
    /// L1 直接答得出來的那幾筆，排在原文前面。
    ///
    /// 這一層以前只長在 `sister query` 裡：於是同一句「電話」，終端機回得出
    /// 號碼、她只會說找不到——螢幕上寫的是「客服**專線**」，全文比對永遠接不
    /// 起那兩個詞。而她才是他每天真的會用的那一個，Phase 1 的退場條件
    /// （「答對我自己都忘掉的東西」）也是拿她量的。
    answers: Vec<Fact>,
    hits: Vec<Hit>,
    /// 底下還有，只是沒送過來。
    ///
    /// `20 筆` 和「一共就這 20 筆」在畫面上長得一模一樣——終端機那邊為了這件
    /// 事在數字後面印一個 `+`，時間軸為了同一件事多撈一筆來判斷
    /// （見 [`DayView::truncated`]）。她這裡以前什麼都沒說：捲到底就是底了，
    /// 而「她只記得這些」正是他會下的結論。
    ///
    /// 只講原文那一半。★ 那一半在 [`answers_truncated`](Self::answers_truncated)。
    truncated: bool,
    /// ★ 答案也被切掉了。
    ///
    /// 這裡以前的說法是「上限 10 筆是**去重後的不同值**——同一個問題有十個
    /// 不同答案的時候，問題出在問法，不是在少給了第十一個」。那句話讀起來
    /// 很有道理，但它把「她只知道這十個」和「她知道更多、只是沒送過來」壓成
    /// 同一個畫面——而這正是隔壁那一欄存在的全部理由。
    answers_truncated: bool,
    /// 一筆都沒找到的時候，她**查得到**的那幾個理由。兩邊都有東西時是 `None`
    /// ——沒答不出來就沒有什麼好解釋的，而且那幾個查詢不必白跑。
    ///
    /// 她原本說的是「這件事我沒看到過」，一句斷言；而正確答案可能是「你自己
    /// 叫我不要看那個網站」。SPEC §8.2 的語氣規範講的就是這個。
    blind: Option<Blind>,
    /// 問句裡認得出來的日曆範圍。`None` = 這句話沒有時間範圍，沒去算章節。
    time_range: Option<AskedTimeRange>,
    /// 那段時間切成的活動級段落。`None` 配 `time_range: None` = 沒算過；
    /// `Some([])` 配有範圍 = 算過，切不出段落。兩種不可以合成一個空陣列。
    ///
    /// 時間軸和答案端都是活動級。分鐘級 `segment` 在時間軸展開才看得到。
    chapters: Option<Vec<Chapter>>,
    /// 「她知道了什麼」專用的 L2 總覽。一般檢索題一定是 `None`。
    ///
    /// 放在最外層而不是塞進 `hits`：L2 是可修正的假設，不是 OCR 原文。兩種東西
    /// 共用一個陣列，renderer 遲早會把其中一種畫成另一種。
    overview: Option<MemoryOverview>,
    /// 本機候選經 CLI 成句後的答案。`None` 時畫面直接使用下面的本機 facts／hits。
    synthesis: Option<GroundedSynthesis>,
    /// 已選的大腦有沒有實際接手這題。畫面只用它給可採取的下一步，不猜 CLI 狀態。
    brain: BrainAnswer,
}

#[derive(Debug, Serialize)]
struct BrainAnswer {
    state: &'static str,
    provider: Option<String>,
}

impl BrainAnswer {
    fn new(state: &'static str, provider: Option<String>) -> Self {
        Self { state, provider }
    }

    fn not_configured() -> Self {
        Self::new("not_configured", None)
    }
}

#[derive(Debug, Serialize)]
struct GroundedSynthesis {
    sentences: Vec<GroundedSentence>,
}

#[derive(Debug, Serialize)]
struct GroundedSentence {
    text: String,
    sources: Vec<GroundedSource>,
}

#[derive(Debug, Serialize)]
struct GroundedSource {
    r#ref: String,
    label: String,
    frame_id: Option<i64>,
}

/// 她已經整理過的記憶，和「沒有整理過」的原因。
///
/// `kind` 是封閉合約；不送一個可空的 `cards` 讓 renderer 自己猜「空」是哪一種。
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MemoryOverview {
    Ready {
        cards: Vec<MemoryOverviewCard>,
        /// 只表示最近四張候選裡，還有一張通過 DB 畫面來源檢查但因三張上限未列。
        /// 不是「已掃過整顆資料庫，而且更舊的卡也都有圖」。
        truncated: bool,
        /// 最近四張候選裡，整張沒有任何 DB 畫面來源可交給 `open_frame` 的卡片數。
        evidence_unavailable: usize,
    },
    RawOnly,
    Empty,
    EvidenceMissing {
        cards: usize,
    },
}

#[derive(Debug, Serialize)]
struct MemoryOverviewCard {
    segment_started_at: i64,
    activity: String,
    author: &'static str,
    model_confidence: f64,
    evidence: Vec<MemoryOverviewEvidence>,
}

#[derive(Debug, Serialize)]
struct MemoryOverviewEvidence {
    frame_id: i64,
    label: String,
}

/// 最近四張 current L2 候選，最多交出三張帶 DB 畫面來源的卡片。
///
/// L2 的 ref 只證明卡片當初指向某筆 L0。交給 `open_frame` 前還要再問一次
/// `frames.image_path`：只記文字、畫面保留期已到、或圖片額度用完時，frame 列仍在，
/// 但不該長出畫面按鈕。這個查詢只驗 DB 宣稱有相對路徑；實體檔若被外部刪掉，
/// `open_frame` 仍會照實失敗，這裡不把它承諾成一定打得開。`fact:` 也只在它仍指得回
/// 一個 frame 時才有畫面資格。
fn memory_overview_from_db(db: &sister_core::db::Db) -> anyhow::Result<MemoryOverview> {
    use sister_core::brain::EvidenceRef;

    const CANDIDATES: usize = 4;
    const SHOWN: usize = 3;

    let candidates = db.recent_l2_cards(CANDIDATES)?;
    if candidates.is_empty() {
        return Ok(if db.has_retained_recorded_content()? {
            MemoryOverview::RawOnly
        } else {
            MemoryOverview::Empty
        });
    }

    struct Candidate {
        card: sister_core::db::L2CardRow,
        evidence: Vec<(i64, String)>,
    }

    let mut resolved = Vec::with_capacity(candidates.len());
    let mut all_frame_ids = Vec::new();
    for card in candidates {
        let refs: Vec<String> = serde_json::from_str(&card.evidence_json).unwrap_or_default();
        let mut evidence = Vec::new();
        for reference in refs.iter().filter_map(|raw| EvidenceRef::parse(raw)) {
            let mapped = match reference {
                EvidenceRef::Frame(frame_id) => Some((frame_id, format!("畫面 #{frame_id}"))),
                EvidenceRef::Fact(fact_id) => db.fact_by_id(fact_id)?.and_then(|fact| {
                    fact.frame_id.map(|frame_id| {
                        (frame_id, format!("本機事實 #{fact_id} 的畫面 #{frame_id}"))
                    })
                }),
            };
            if let Some((frame_id, label)) = mapped {
                // 同一張畫面可能同時被 `frame:` 和 `fact:` 指到。按鈕的能力相同，
                // 一張卡裡不必用兩顆看似不同的按鈕冒充兩份畫面證據。
                if evidence
                    .iter()
                    .any(|(known, _): &(i64, String)| *known == frame_id)
                {
                    continue;
                }
                all_frame_ids.push(frame_id);
                evidence.push((frame_id, label));
            }
        }
        resolved.push(Candidate { card, evidence });
    }

    let openable = db.frames_with_image(&all_frame_ids)?;
    let mut cards = Vec::new();
    let mut evidence_unavailable = 0;
    let mut additional_openable = false;
    for candidate in resolved {
        let evidence: Vec<MemoryOverviewEvidence> = candidate
            .evidence
            .into_iter()
            .filter(|(frame_id, _)| openable.contains(frame_id))
            .map(|(frame_id, label)| MemoryOverviewEvidence { frame_id, label })
            .collect();
        if evidence.is_empty() {
            evidence_unavailable += 1;
            continue;
        }
        if cards.len() == SHOWN {
            additional_openable = true;
            continue;
        }
        cards.push(MemoryOverviewCard {
            segment_started_at: candidate.card.segment_core_start,
            activity: candidate.card.activity,
            author: candidate.card.author.as_str(),
            model_confidence: candidate.card.model_confidence,
            evidence,
        });
    }

    if cards.is_empty() {
        Ok(MemoryOverview::EvidenceMissing {
            cards: evidence_unavailable,
        })
    } else {
        Ok(MemoryOverview::Ready {
            cards,
            truncated: additional_openable,
            evidence_unavailable,
        })
    }
}

/// 建立總覽回答，但不把它冒充成 retrieval query。
///
/// `queries`、`query_clicks` 與 `query_marks` 是檢索品質的題庫；總覽讀的是 current
/// L2，沒有 retrieval rank、chunk click 或可重播的 FTS 問法。這版沒有另一套總覽
/// 評測 schema，因此 `query_id` 必須明確是 `None`，也不讀 query-log 設定。
fn memory_overview_answer(
    db: &sister_core::db::Db,
    started: std::time::Instant,
) -> Result<Answer, String> {
    let overview = memory_overview_from_db(db).map_err(|e| format!("{e:#}"))?;
    let shown = match &overview {
        MemoryOverview::Ready { cards, .. } => cards.len(),
        MemoryOverview::RawOnly
        | MemoryOverview::Empty
        | MemoryOverview::EvidenceMissing { .. } => 0,
    };
    tracing::info!(
        "問了一次（記憶總覽）：{} 張帶資料庫畫面來源的理解，{} ms",
        shown,
        started.elapsed().as_millis()
    );

    Ok(Answer {
        presentation_id: None,
        kind: sister_core::question::Intent::MemoryOverview.name(),
        followup: None,
        closure_notice: None,
        searched: None,
        query_id: None,
        answers: Vec::new(),
        hits: Vec::new(),
        truncated: false,
        answers_truncated: false,
        blind: None,
        time_range: None,
        chapters: None,
        overview: Some(overview),
        synthesis: None,
        brain: BrainAnswer::not_configured(),
    })
}

#[cfg(test)]
mod memory_overview_tests {
    use super::*;
    use sister_core::db::{Db, L2Author, L2Insert};
    use sister_core::model::{FocusSnapshot, FrameCapture, OcrBlock};
    use std::sync::atomic::{AtomicU32, Ordering};

    const SETTINGS_OCR: &str = "這一欄和其他每一欄不一樣：其他的是她看到的東西，這一欄是你自己打進去的字。留著是為了知道她哪些題答不出來——答不出來的那些，才是下一版要修的。時間軸上「忘掉這一段」會一併帶走，過期規則跟文字一樣。";

    fn insert_frame(
        db: &mut Db,
        session: i64,
        ts: i64,
        text: &str,
        image_path: Option<&str>,
    ) -> i64 {
        let frame = FrameCapture {
            ts,
            monitor: 0,
            width: 1920,
            height: 1080,
            dhash: ts as u64,
            image: None,
            image_ext: "png",
            ocr: vec![OcrBlock {
                text: text.to_string(),
                x: 0,
                y: 0,
                w: 800,
                h: 40,
                confidence: 0.9,
            }],
            focus: FocusSnapshot {
                app_id: Some("sister-desktop.exe".into()),
                window_title: Some("AI-Sister 設定".into()),
                ..Default::default()
            },
        };
        db.insert_frame(session, &frame, image_path, i64::from(image_path.is_some()))
            .expect("insert frame")
            .0
    }

    fn insert_card(db: &mut Db, segment: i64, activity: &str, refs: &[String]) -> i64 {
        db.insert_l2_card(&L2Insert {
            segment_core_start: segment,
            segment_ref: &format!("segment:{segment}"),
            activity,
            entities_json: "[]".into(),
            continues_json: None,
            commitments_json: "[]".into(),
            model_confidence: 0.73,
            evidence_json: serde_json::to_string(refs).expect("serialize refs"),
            open_questions_json: "[]".into(),
            author: L2Author::Interpreter,
        })
        .expect("insert L2")
    }

    fn json(overview: &MemoryOverview) -> serde_json::Value {
        serde_json::to_value(overview).expect("serialize overview")
    }

    #[test]
    fn no_l2_distinguishes_empty_from_raw_only_without_returning_settings_ocr() {
        let mut db = Db::open_in_memory().expect("open db");
        assert_eq!(
            json(&memory_overview_from_db(&db).unwrap())["kind"],
            "empty"
        );

        let session = db.start_session("test", "test").expect("start session");
        insert_frame(&mut db, session, 1_000, SETTINGS_OCR, None);
        let positive = db.search("知道", 10).expect("positive-control FTS");
        assert_eq!(positive.len(), 1, "測試必須真的建出舊路徑會撈到的設定 OCR");
        assert_eq!(positive[0].text, SETTINGS_OCR);

        let overview = memory_overview_from_db(&db).expect("raw-only overview");
        let serialized = json(&overview);
        assert_eq!(serialized["kind"], "raw_only");
        assert!(
            !serialized.to_string().contains(SETTINGS_OCR),
            "raw-only 只能說尚未整理，不能把 positive-control OCR 送回去：{serialized}"
        );
    }

    #[test]
    fn overview_answer_never_enters_the_retrieval_query_tables() {
        let db = Db::open_in_memory().expect("open db");
        db.log_query(&sister_core::db::QueryLogEntry {
            ts: 1,
            question: "既有的檢索題",
            shape: "keywords",
            hits: 1,
            latency_ms: 2,
            source: sister_core::db::SOURCE_DESKTOP,
        })
        .expect("seed retrieval query");
        let before = db.query_log_stats().expect("stats before overview");

        let answer =
            memory_overview_answer(&db, std::time::Instant::now()).expect("memory overview answer");
        let after = db.query_log_stats().expect("stats after overview");

        assert_eq!(answer.kind, "memory_overview");
        assert_eq!(answer.query_id, None);
        assert_eq!(after, before, "總覽不可污染 retrieval 的任何分母");
        assert_eq!(db.query_log(10).expect("query rows").len(), 1);
    }

    #[test]
    fn ready_uses_fact_frame_and_never_returns_raw_ocr_as_the_answer() {
        let mut db = Db::open_in_memory().expect("open db");
        let session = db.start_session("test", "test").expect("start session");
        let frame_id = insert_frame(
            &mut db,
            session,
            2_000,
            &format!("{SETTINGS_OCR}\n客服專線 0800-080-123"),
            Some("ready.png"),
        );
        let positive = db.search("知道", 10).expect("positive-control FTS");
        assert_eq!(
            positive.len(),
            1,
            "舊的 keywords 路徑在這顆 DB 必須確實命中"
        );
        let fact_id = db
            .fact_sightings("phone", 10)
            .expect("phone facts")
            .into_iter()
            .next()
            .expect("phone fact")
            .0
            .id;
        insert_card(
            &mut db,
            2_000,
            "正在修安裝更新",
            &[format!("fact:{fact_id}")],
        );

        let overview = memory_overview_from_db(&db).expect("ready overview");
        let serialized = json(&overview);
        assert_eq!(serialized["kind"], "ready");
        assert_eq!(serialized["cards"].as_array().map(Vec::len), Some(1));
        assert_eq!(serialized["cards"][0]["activity"], "正在修安裝更新");
        assert_eq!(serialized["cards"][0]["evidence"][0]["frame_id"], frame_id);
        assert!(
            serialized["cards"][0]["evidence"][0]["label"]
                .as_str()
                .is_some_and(|label| label.contains(&format!("本機事實 #{fact_id}"))),
            "fact ref 的來源身分不能在映成 frame 後消失：{serialized}"
        );
        assert!(
            !serialized.to_string().contains(SETTINGS_OCR),
            "ready 只送 L2 activity，不送 positive-control OCR：{serialized}"
        );
    }

    #[test]
    fn ready_uses_the_current_user_correction_with_its_inherited_evidence() {
        let mut db = Db::open_in_memory().expect("open db");
        let session = db.start_session("test", "test").expect("start session");
        let frame_id = insert_frame(
            &mut db,
            session,
            3_000,
            "畫面上的原始脈絡",
            Some("corrected.png"),
        );
        insert_card(
            &mut db,
            3_000,
            "SUPERSEDED_MODEL_ACTIVITY_MUST_NOT_RETURN",
            &[format!("frame:{frame_id}")],
        );
        sister_core::reviewer::correct_l2(&mut db, 3_000, "這是我自己修正的說法")
            .expect("correct L2");

        let serialized = json(&memory_overview_from_db(&db).expect("corrected overview"));
        assert_eq!(serialized["kind"], "ready");
        assert_eq!(serialized["cards"].as_array().map(Vec::len), Some(1));
        assert_eq!(serialized["cards"][0]["activity"], "這是我自己修正的說法");
        assert_eq!(serialized["cards"][0]["author"], "user");
        assert_eq!(serialized["cards"][0]["evidence"][0]["frame_id"], frame_id);
        assert!(
            !serialized
                .to_string()
                .contains("SUPERSEDED_MODEL_ACTIVITY_MUST_NOT_RETURN"),
            "總覽只能回 current user version，不能把 superseded model activity 混回來"
        );
    }

    #[test]
    fn ready_caps_at_three_and_counts_whole_withheld_cards() {
        let mut db = Db::open_in_memory().expect("open db");
        let session = db.start_session("test", "test").expect("start session");
        for segment in 1..=4 {
            let frame_id = insert_frame(
                &mut db,
                session,
                segment * 1_000,
                &format!("card {segment}"),
                Some("open.png"),
            );
            insert_card(
                &mut db,
                segment * 1_000,
                &format!("activity {segment}"),
                &[format!("frame:{frame_id}")],
            );
        }
        let serialized = json(&memory_overview_from_db(&db).expect("overview"));
        assert_eq!(serialized["cards"].as_array().map(Vec::len), Some(3));
        assert_eq!(serialized["truncated"], true);
        assert_eq!(serialized["evidence_unavailable"], 0);

        let mut db = Db::open_in_memory().expect("open withheld db");
        let session = db.start_session("test", "test").expect("start session");
        for segment in 1..=2 {
            let frame_id =
                insert_frame(&mut db, session, segment * 1_000, "open", Some("open.png"));
            insert_card(
                &mut db,
                segment * 1_000,
                &format!("shown {segment}"),
                &[format!("frame:{frame_id}")],
            );
        }
        insert_card(&mut db, 3_000, "missing refs", &["frame:999999".into()]);
        insert_card(&mut db, 4_000, "malformed refs", &["not-a-ref".into()]);
        let serialized = json(&memory_overview_from_db(&db).expect("withheld overview"));
        assert_eq!(serialized["cards"].as_array().map(Vec::len), Some(2));
        assert_eq!(serialized["truncated"], false);
        assert_eq!(
            serialized["evidence_unavailable"], 2,
            "算的是整張 withheld 卡，不是卡裡壞了幾個 ref"
        );
    }

    #[test]
    fn invalid_current_version_does_not_revive_a_superseded_card() {
        let mut db = Db::open_in_memory().expect("open db");
        let session = db.start_session("test", "test").expect("start session");
        let frame_id = insert_frame(&mut db, session, 5_000, "old evidence", Some("old.png"));
        insert_card(
            &mut db,
            5_000,
            "SUPERSEDED_ACTIVITY_MUST_NOT_RETURN",
            &[format!("frame:{frame_id}")],
        );
        insert_card(
            &mut db,
            5_000,
            "current but missing evidence",
            &["frame:999999".into()],
        );

        let serialized = json(&memory_overview_from_db(&db).expect("overview"));
        assert_eq!(serialized["kind"], "evidence_missing");
        assert_eq!(serialized["cards"], 1);
        assert!(
            !serialized
                .to_string()
                .contains("SUPERSEDED_ACTIVITY_MUST_NOT_RETURN")
        );
    }

    #[test]
    fn evidence_without_an_openable_image_is_not_an_answer() {
        let mut db = Db::open_in_memory().expect("open db");
        let session = db.start_session("test", "test").expect("start session");
        let frame_id = insert_frame(&mut db, session, 6_000, "text-only", None);
        insert_card(
            &mut db,
            6_000,
            "card whose frame is text-only",
            &[format!("frame:{frame_id}")],
        );

        let serialized = json(&memory_overview_from_db(&db).expect("overview"));
        assert_eq!(serialized["kind"], "evidence_missing");
        assert_eq!(serialized["cards"], 1);
    }

    #[test]
    fn forgetting_the_evidence_removes_the_overview_card() {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "ai-sister-overview-forget-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create frame root");
        std::fs::write(root.join("forgotten.png"), b"pixel").expect("seed image");

        let mut db = Db::open_in_memory().expect("open db");
        let session = db.start_session("test", "test").expect("start session");
        let frame_id = insert_frame(
            &mut db,
            session,
            7_000,
            "forget this source",
            Some("forgotten.png"),
        );
        insert_card(
            &mut db,
            7_000,
            "FORGOTTEN_ACTIVITY_MUST_NOT_RETURN",
            &[format!("frame:{frame_id}")],
        );
        assert_eq!(
            json(&memory_overview_from_db(&db).expect("before forget"))["kind"],
            "ready"
        );

        db.forget(7_000, 7_001, Some(&root)).expect("forget range");
        let serialized = json(&memory_overview_from_db(&db).expect("after forget"));
        assert_eq!(serialized["kind"], "empty");
        assert!(
            !serialized
                .to_string()
                .contains("FORGOTTEN_ACTIVITY_MUST_NOT_RETURN")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// [`Answer::time_range`]：回述用的是他原話裡的那一段。
#[derive(Serialize)]
struct AskedTimeRange {
    from: i64,
    to: i64,
    said: String,
}

/// [`Answer::blind`] 的內容。核心那份（`sister_core::answer::BlindSpots`）只回
/// 事實，句子由這一頁自己組——終端機和字母人的講法不一樣，根據是同一份。
#[derive(Serialize)]
struct Blind {
    /// 她一共記過幾段文字。`0` **不等於**「還沒開始記」，也**不等於**
    /// 「OCR 沒讀到東西」——見 [`sister_core::answer::BlindSpots::chunks`]。
    chunks: i64,
    /// 留了畫面卻一行字都沒讀出來。
    ///
    /// 送一個**布林**而不是 `ocr_blocks` 的數字：門檻（幾張畫面才算數）是
    /// 核心那邊的判斷，兩邊各寫一次的話遲早有一邊改了另一邊沒改，而這一句
    /// 正好是這個專案已知的主要故障形狀唯一會被講出口的地方。見
    /// [`sister_core::answer::BlindSpots::ocr_is_dead`]。
    ocr_is_dead: bool,
    /// 她一共留下幾張畫面。`chunks == 0 && frames > 0` = 她看了，
    /// 但一個字都沒讀出來（讀字那一段斷了）。
    frames: i64,
    /// 她**曾經**開始記過東西嗎。兩個 0 配上 `true` = 錄過、但被忘掉了，
    /// 不是還沒開始——見 [`sister_core::answer::BlindSpots::ever_recorded`]。
    ever_recorded: bool,
    /// 她有沒有**真的存下來過一列內容**。上面那個位元在 `start_session` 就
    /// 翻成 true，第一張畫面之前——所以一台 `capture.enabled = false` 的機器
    /// 兩個 0 也配得到 `ever_recorded: true`，然後被告知東西被忘掉了。見
    /// [`sister_core::answer::BlindSpots::ever_stored`]。
    ever_stored: bool,
    /// 排除規則生效過的（理由, 段數）。段不是張。
    excluded: Vec<(String, i64)>,
    paused_episodes: i64,
    /// **只含已結束的那幾段。**`0` 配上 `paused_open` 的意思是「三天前按下去
    /// 到現在都沒解除」，不是「暫停了一瞬間」——見
    /// [`sister_core::answer::BlindSpots::paused_open`]。
    paused_ms: i64,
    paused_open: bool,
    /// 她**此刻**閉著眼睛沒有。和 `paused_open` 是兩件事：那一個講的是
    /// 資料庫裡最後一筆暫停有沒有配到解除，而暫停中關掉 recorder、事後才
    /// 解除的人，會永遠掛著一筆配不到的——見
    /// [`sister_core::answer::BlindSpots::paused_now`]。
    paused_now: bool,
    paused_truncated: i64,
    /// recorder／brain／hands 三層一起全停過幾段。這是資料庫歷史，不是現在式。
    master_stopped_episodes: i64,
    /// 已結束的全停段落總長；有 open／truncated 時只是已知下限。
    master_stopped_ms: i64,
    /// 最後一段全停沒有對應的解除稽核列；不拿它猜 durable latch 現在是否仍開著。
    master_stopped_open: bool,
    /// 有幾段只剩解除列，開頭已被保留期刪除。
    master_stopped_truncated: i64,
    /// `clear`／`stopping`／`stopped`／`uncertain`；pending 絕不冒充已排乾完成。
    master_stop_state: sister_hands::master_stop::State,
    /// 這一題只翻了最近幾天。`null` = 整顆資料庫都翻過了。見
    /// [`sister_core::answer::BlindSpots::scan_horizon_days`]。
    scan_horizon_days: Option<i64>,
    /// 現在有沒有人在錄。只在「一段字都沒有」那組句子裡用得到，而它在那裡
    /// 分開的是「被忘掉了／過期了」和「她三秒前才開起來」——見
    /// [`sister_core::answer::BlindSpots::recording_now`]。
    recording_now: bool,
    /// 有一個 recorder **正在起來**，還沒開始錄。和上面那個是同一次心跳讀出
    /// 來的兩半，永遠不會同時為真——見
    /// [`sister_core::answer::BlindSpots::booting_now`]。
    ///
    /// 少了這一格，開機那幾分鐘字母人會說「先看設定頁的『開始記錄』那一段」，
    /// 對一個什麼都還沒開始的 recorder。
    booting_now: bool,
}

impl From<sister_core::answer::BlindSpots> for Blind {
    fn from(blind: sister_core::answer::BlindSpots) -> Self {
        let ocr_is_dead = blind.ocr_is_dead();
        Self {
            chunks: blind.chunks,
            ocr_is_dead,
            frames: blind.frames,
            ever_recorded: blind.ever_recorded,
            ever_stored: blind.ever_stored,
            excluded: blind.excluded,
            paused_episodes: blind.paused_episodes,
            paused_ms: blind.paused_ms,
            paused_open: blind.paused_open,
            paused_now: blind.paused_now,
            paused_truncated: blind.paused_truncated,
            master_stopped_episodes: blind.master_stopped_episodes,
            master_stopped_ms: blind.master_stopped_ms,
            master_stopped_open: blind.master_stopped_open,
            master_stopped_truncated: blind.master_stopped_truncated,
            master_stop_state: blind.master_stop_state,
            scan_horizon_days: blind.scan_horizon_days,
            recording_now: blind.recording_now,
            booting_now: blind.booting_now,
        }
    }
}

#[cfg(test)]
mod blind_dto_tests {
    use super::*;

    #[test]
    fn core_blind_spots_serialize_to_the_exact_ipc_fields() {
        let dto = Blind::from(sister_core::answer::BlindSpots {
            chunks: 11,
            ocr_blocks: 12,
            frames: 13,
            ever_recorded: true,
            ever_stored: false,
            excluded: vec![("excluded app: fixture".to_string(), 14)],
            paused_episodes: 17,
            paused_ms: 19,
            paused_open: false,
            paused_now: false,
            paused_truncated: 23,
            master_stop_state: sister_hands::master_stop::State::Stopping,
            master_stopped_episodes: 29,
            master_stopped_ms: 31,
            master_stopped_open: true,
            master_stopped_truncated: 37,
            scan_horizon_days: Some(41),
            recording_now: true,
            booting_now: false,
        });

        assert_eq!(
            serde_json::to_value(dto).expect("serialize Blind IPC DTO"),
            serde_json::json!({
                "chunks": 11,
                "ocr_is_dead": false,
                "frames": 13,
                "ever_recorded": true,
                "ever_stored": false,
                "excluded": [["excluded app: fixture", 14]],
                "paused_episodes": 17,
                "paused_ms": 19,
                "paused_open": false,
                "paused_now": false,
                "paused_truncated": 23,
                "master_stopped_episodes": 29,
                "master_stopped_ms": 31,
                "master_stopped_open": true,
                "master_stopped_truncated": 37,
                "master_stop_state": "stopping",
                "scan_horizon_days": 41,
                "recording_now": true,
                "booting_now": false,
            })
        );
    }
}

/// 一筆 ★ 答案。
#[derive(Serialize)]
struct Fact {
    fact_id: i64,
    /// 正規化後的值——`+886800080123`，不是螢幕上那串 `0800-080-123`。
    value: String,
    /// 螢幕上真正長的樣子。兩個都給：正規化後的值認得出來，原文才認得出**場景**。
    ///
    /// 也是 `value` 難讀時的救生索：金額正規化成 `TWD:13450`，而他記得的是
    /// 這一行裡的「帳單 NT$13,450」。
    raw: String,
    /// 看過幾次。1 次和 12 次是不同強度的答案，而她自己不做判斷，只把數字講出來。
    ///
    /// 數的是**遇到過幾次**，不是資料庫裡有幾列——一列是一張留下來的畫面，
    /// 而盯著同一個視窗二十分鐘會留下三百張。見 `Db::SAME_SITTING_MS`。
    sightings: usize,
    ts: i64,
    /// 這個值是從哪一段字抽出來的。點開出處時要掛回題庫（見 `log_click`）。
    /// `None` 的那些照樣點得開，只是那一下不會被記成正解。
    chunk_id: Option<i64>,
    frame_id: Option<i64>,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

fn synthesis_from_grounded(
    grounded: sister_core::grounded_answer::GroundedAnswer,
    facts: &[Fact],
    hits: &[Hit],
) -> Result<GroundedSynthesis, String> {
    use sister_core::grounded_answer::SourceRef;

    let sentences = grounded
        .sentences
        .into_iter()
        .map(|sentence| {
            let sources = sentence
                .sources
                .into_iter()
                .map(|reference| match reference {
                    SourceRef::Fact(id) => {
                        let fact = facts
                            .iter()
                            .find(|fact| fact.fact_id == id)
                            .ok_or_else(|| format!("找不到回答引用的本機事實 #{id}"))?;
                        Ok(GroundedSource {
                            r#ref: reference.as_str(),
                            label: fact.frame_id.map_or_else(
                                || format!("事實 #{id}"),
                                |frame| format!("畫面 #{frame}"),
                            ),
                            frame_id: fact.frame_id,
                        })
                    }
                    SourceRef::Chunk(id) => {
                        let hit = hits
                            .iter()
                            .find(|hit| hit.chunk_id == id)
                            .ok_or_else(|| format!("找不到回答引用的本機文字 #{id}"))?;
                        Ok(GroundedSource {
                            r#ref: reference.as_str(),
                            label: hit.frame_id.map_or_else(
                                || format!("文字 #{id}"),
                                |frame| format!("畫面 #{frame}"),
                            ),
                            frame_id: hit.frame_id,
                        })
                    }
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(GroundedSentence {
                text: sentence.text,
                sources,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(GroundedSynthesis { sentences })
}

#[cfg(test)]
mod grounded_synthesis_tests {
    use super::*;
    use sister_core::grounded_answer::{GroundedAnswer, GroundedSentence, SourceRef};

    fn fact() -> Fact {
        Fact {
            fact_id: 9,
            value: "+886800080123".into(),
            raw: "客服專線 0800-080-123".into(),
            sightings: 2,
            ts: 100,
            chunk_id: Some(31),
            frame_id: Some(42),
            app: Some("chrome.exe".into()),
            title: Some("帳單".into()),
            url: None,
        }
    }

    fn hit() -> Hit {
        Hit {
            chunk_id: 77,
            ts: 200,
            text: "昨天完成匯出".into(),
            snippet: "昨天完成匯出".into(),
            app: Some("notes.exe".into()),
            title: Some("工作筆記".into()),
            url: None,
            frame_id: None,
        }
    }

    #[test]
    fn grounded_refs_map_only_to_the_same_local_answer_sources() {
        let synthesis = synthesis_from_grounded(
            GroundedAnswer {
                sentences: vec![GroundedSentence {
                    text: "客服電話是 0800-080-123。".into(),
                    sources: vec![SourceRef::Fact(9), SourceRef::Chunk(77)],
                }],
            },
            &[fact()],
            &[hit()],
        )
        .unwrap();
        assert_eq!(synthesis.sentences.len(), 1);
        assert_eq!(synthesis.sentences[0].sources[0].r#ref, "fact:9");
        assert_eq!(synthesis.sentences[0].sources[0].frame_id, Some(42));
        assert_eq!(synthesis.sentences[0].sources[1].r#ref, "chunk:77");
        assert_eq!(synthesis.sentences[0].sources[1].frame_id, None);
    }

    #[test]
    fn a_ref_missing_from_the_local_answer_rejects_the_whole_synthesis() {
        let grounded = GroundedAnswer {
            sentences: vec![GroundedSentence {
                text: "沒有這筆來源。".into(),
                sources: vec![SourceRef::Chunk(999)],
            }],
        };
        assert!(synthesis_from_grounded(grounded, &[fact()], &[hit()]).is_err());
    }
}

#[derive(Debug, Clone)]
struct ConfiguredAnswerCli {
    command: String,
    args: Vec<String>,
    label: String,
}

#[derive(Debug)]
struct PlannedAnswerSearches {
    cli: ConfiguredAnswerCli,
    queries: Vec<String>,
}

fn answer_cli_from_config(config: &sister_core::config::Config) -> Option<ConfiguredAnswerCli> {
    let (command, args) = config.brain.cli()?;
    // 互動問答只接受設定頁完成登入、固定 probe 並明確選用的 bridge。舊 raw
    // command 仍留給 recorder 相容使用，但不能在沒有選用 provider 的情況下
    // 悄悄接管輸入框。
    let (provider, _) = sister_core::provider_cli::parse_bridge_args(args)?;
    let label = provider.label().to_owned();
    Some(ConfiguredAnswerCli {
        command: command.to_owned(),
        args: args.to_vec(),
        label,
    })
}

fn configured_answer_cli() -> Result<Option<ConfiguredAnswerCli>, String> {
    let path = config_path()?;
    let config = sister_core::config::Config::load(&path).map_err(|error| format!("{error:#}"))?;
    Ok(answer_cli_from_config(&config))
}

#[cfg(test)]
mod answer_cli_selection_tests {
    use super::*;
    use sister_core::provider_cli::{BrainProvider, bridge_args};

    #[test]
    fn the_last_provider_selected_in_config_is_the_answer_cli() {
        let mut config = sister_core::config::Config::default();
        config.set_brain_cli_from_page(
            "sister".into(),
            bridge_args(BrainProvider::Claude, Path::new("claude")),
        );
        assert_eq!(
            answer_cli_from_config(&config).unwrap().label,
            "Claude Code"
        );

        config.set_brain_cli_from_page(
            "sister".into(),
            bridge_args(BrainProvider::Grok, Path::new("grok")),
        );
        let selected = answer_cli_from_config(&config).unwrap();
        assert_eq!(selected.label, "Grok CLI");
        assert_eq!(selected.command, "sister");
        assert_eq!(
            sister_core::provider_cli::parse_bridge_args(&selected.args)
                .map(|(provider, _)| provider),
            Some(BrainProvider::Grok)
        );

        config.set_brain_cli_from_page("custom-agent".into(), vec!["--raw".into()]);
        assert!(answer_cli_from_config(&config).is_none());
    }
}

/// 畫面回報一則觀測。
///
/// **native 只存，不算。** 這一格叫什麼、算不算「照你要的」，全在
/// [`sister_core::diagnose::Notebook::items`] 那一邊；畫面送得出來的只有數字、
/// 旗標和一個 `lineId`（見 `Note` 的型別）。少了這條分工，畫面就變成報告的第
/// 二個寫入端，而那份報告裡有一半的字會是它說了算——那正好是這份報告最不該
/// 有的性質。
///
/// 不回錯誤：一則診斷觀測沒記成，不可以讓她的畫面出事。
#[tauri::command]
fn diagnose_note(shell: tauri::State<'_, Shell>, note: sister_core::diagnose::Note) {
    if let Ok(mut book) = shell.diagnostics.lock() {
        book.note(note);
    }
}

/// 把這一輪寫成一個可以貼出去的檔案，回傳它在哪。
///
/// 讀什麼、印什麼和 `sister diagnose` 是同一支 `collect_from` 加同一支
/// `render`；這裡多的只有畫面那兩節（自檢、答案原文），以及「寫到哪」。
#[tauri::command]
fn diagnose_export(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<String, String> {
    let data_dir = shell
        .data_dir
        .clone()
        .ok_or_else(|| "這台機器上問不出資料目錄，寫不出診斷報告。".to_string())?;
    let now = sister_core::now_ms();

    // 用她已經開著的那條連線。再開第二條會在她正在寫的時候撞上 migration
    // 檢查，而一份「只有在忙的時候才查不出來」的診斷報告，剛好在最需要它的
    // 那一刻失效。開不起來也不是致命的——那幾格會誠實地說自己查不出來。
    let mut snapshot = with_db(&shell, |db| {
        Ok(sister_core::diagnose::collect_from(
            &data_dir,
            "桌面版設定頁",
            env!("CARGO_PKG_VERSION"),
            Ok(db),
        ))
    })
    .unwrap_or_else(|_| {
        // `with_db` 把「還沒有任何記憶」和「開不起來」收成同一個字串，而這份
        // 報告存在的理由就是分得出這兩件事：一格印「問不出來」而真相是「他還
        // 沒錄過」，正好是它自己反對的那種話。所以這裡自己看一眼那個檔在不在。
        let why = if sister_core::config::Config::db_path(&data_dir).exists() {
            sister_core::diagnose::Absent::QueryFailed
        } else {
            sister_core::diagnose::Absent::NotThere
        };
        sister_core::diagnose::collect_from(
            &data_dir,
            "桌面版設定頁",
            env!("CARGO_PKG_VERSION"),
            Err(why),
        )
    });

    if let Ok(book) = shell.diagnostics.lock() {
        book.fill(&mut snapshot, now);
    }

    let text = sister_core::diagnose::render(&snapshot);
    let path = diagnose_report_dir(&app).join(sister_core::diagnose::file_name(now));
    std::fs::write(&path, &text).map_err(|error| format!("寫不進 {}：{error}", path.display()))?;
    tracing::info!("診斷報告寫到 {}", path.display());
    Ok(path.display().to_string())
}

/// 報告寫到看得到的地方。
///
/// 最後才退到暫存資料夾，**不退到 data dir**：那裡每多一個檔案，三條刪除路
/// 就各要接一次。
fn diagnose_report_dir(app: &tauri::AppHandle) -> PathBuf {
    let resolver = app.path();
    for dir in [
        resolver.desktop_dir(),
        resolver.download_dir(),
        resolver.document_dir(),
        resolver.home_dir(),
    ]
    .into_iter()
    .flatten()
    {
        if dir.is_dir() {
            return dir;
        }
    }
    std::env::temp_dir()
}

fn record_answer_outbound(
    shell: &tauri::State<'_, Shell>,
    cli: &ConfiguredAnswerCli,
    spawn: &sister_core::brain::SpawnOutcome,
    truncated: bool,
    outcome: &str,
    error: Option<&str>,
    role: &str,
) -> Result<(), String> {
    let day = sister_core::brain::local_day_key(sister_core::now_ms())
        .ok_or_else(|| "算不出外送日期".to_owned())?;
    with_db_mut(shell, |db| {
        db.insert_brain_outbound(&sister_core::db::OutboundInsert {
            ts: sister_core::now_ms(),
            day_key: &day,
            command: &cli.command,
            args: &cli.args,
            segment_core_start: None,
            chars_sent: spawn.payload_chars_written as i64,
            truncated,
            outcome,
            duration_ms: spawn.duration_ms as i64,
            error,
            role,
        })
        .map_err(|error| format!("{error:#}"))?;
        Ok(())
    })
}

/// 每個問題先讓已選 CLI 寫出最多三條本機記憶查詢。CLI 只收到問題；SQLite 路徑、
/// SQL 與整顆資料庫都不交出去，查詢由 AI-Sister 在本機執行。
fn plan_answer_searches(
    shell: &tauri::State<'_, Shell>,
    question: &str,
    cancellation: &sister_core::brain::Cancellation,
) -> (BrainAnswer, Option<PlannedAnswerSearches>) {
    let cli = match configured_answer_cli() {
        Ok(Some(cli)) => cli,
        Ok(None) => return (BrainAnswer::not_configured(), None),
        Err(error) => {
            tracing::warn!("讀不到答題 CLI 設定：{error}");
            return (BrainAnswer::new("search_failed", None), None);
        }
    };
    let status = |state| BrainAnswer::new(state, Some(cli.label.clone()));
    let Some(data_dir) = shell.data_dir.as_deref() else {
        return (status("search_failed"), None);
    };
    let consent = sister_core::consent::load(data_dir);
    let Some(permit) = consent.cloud_permit() else {
        return (status("consent_required"), None);
    };
    let payload = match sister_core::grounded_answer::prepare_search_plan(question) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!("答題大腦無法準備記憶查詢：{error:#}");
            return (status("search_failed"), None);
        }
    };
    let Some(not_stopped) = sister_core::brain::not_stopped(data_dir) else {
        return (status("search_failed"), None);
    };
    let spawn = sister_core::brain::spawn_cli_cancellable(
        permit,
        not_stopped,
        &payload,
        &cli.command,
        &cli.args,
        cancellation,
    );

    let (outcome, queries, error) = if cancellation.is_cancelled() {
        ("cancelled", None, None)
    } else if spawn.timed_out {
        ("timeout", None, None)
    } else if !spawn.completed_the_ask() {
        let error = spawn.spawn_error.clone().or_else(|| {
            Some(format!(
                "CLI 結束碼 {}",
                spawn
                    .exit_code
                    .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
            ))
        });
        ("spawn_failed", None, error)
    } else if spawn.stdout.trim().is_empty() {
        ("no_answer", None, None)
    } else {
        match sister_core::grounded_answer::parse_search_plan(&spawn.stdout) {
            Ok(queries) => ("success", Some(queries), None),
            Err(error) => ("bad_json", None, Some(error)),
        }
    };

    if let Err(audit_error) = record_answer_outbound(
        shell,
        &cli,
        &spawn,
        question.len() > sister_core::grounded_answer::MAX_QUESTION_BYTES,
        outcome,
        error.as_deref(),
        "answer_search",
    ) {
        tracing::error!("答題大腦的記憶查詢紀錄沒有寫成：{audit_error}");
        return (status("search_failed"), None);
    }

    match queries {
        Some(queries) => (
            status("searching"),
            Some(PlannedAnswerSearches { cli, queries }),
        ),
        None => (status("search_failed"), None),
    }
}

/// CLI 已經決定並完成本機 retrieval；這一段只讓同一支 CLI 根據命中的證據成句。
/// 每一條失敗路都保留 facts／hits，而且第二次送出仍重新通過 consent 與 master stop。
fn synthesize_grounded_answer(
    shell: &tauri::State<'_, Shell>,
    local: &Answer,
    prepared: &sister_core::grounded_answer::Prepared,
    cli: &ConfiguredAnswerCli,
    cancellation: &sister_core::brain::Cancellation,
    presentation: &sister_hands::master_stop::ActivityGuard,
) -> Option<GroundedSynthesis> {
    let run = || -> Result<Option<GroundedSynthesis>, String> {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        let data_dir = shell
            .data_dir
            .as_deref()
            .ok_or_else(|| "找不到資料目錄".to_owned())?;
        let consent = sister_core::consent::load(data_dir);
        let Some(permit) = consent.cloud_permit() else {
            return Ok(None);
        };
        let Some(not_stopped) = sister_core::brain::not_stopped(data_dir) else {
            return Ok(None);
        };

        let spawn = sister_core::brain::spawn_cli_cancellable(
            permit,
            not_stopped,
            &prepared.payload,
            &cli.command,
            &cli.args,
            cancellation,
        );
        let (mut outcome, mut synthesis, mut error) = if cancellation.is_cancelled() {
            ("cancelled", None, None)
        } else if spawn.timed_out {
            ("timeout", None, None)
        } else if !spawn.completed_the_ask() {
            let error = spawn.spawn_error.clone().or_else(|| {
                Some(format!(
                    "CLI 結束碼 {}",
                    spawn
                        .exit_code
                        .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
                ))
            });
            ("spawn_failed", None, error)
        } else if spawn.stdout.trim().is_empty() {
            ("no_answer", None, None)
        } else {
            match sister_core::grounded_answer::parse(&spawn.stdout, &prepared.sources) {
                Ok(answer) => match synthesis_from_grounded(answer, &local.answers, &local.hits) {
                    Ok(answer) => ("success", Some(answer), None),
                    Err(error) => ("bad_json", None, Some(error)),
                },
                Err(error) => ("bad_json", None, Some(error)),
            }
        };

        if cancellation.is_cancelled() {
            outcome = "cancelled";
            synthesis = None;
            error = None;
        }

        let audit = record_answer_outbound(
            shell,
            cli,
            &spawn,
            prepared.truncated,
            outcome,
            error.as_deref(),
            "answer",
        );
        if let Err(error) = audit {
            tracing::error!("答題層外送稽核沒有寫成：{error}");
            return Ok(None);
        }

        let Some(synthesis) = synthesis else {
            return Ok(None);
        };
        if cancellation.is_cancelled() || presentation.boundary().is_none() {
            return Ok(None);
        }
        Ok(Some(synthesis))
    };

    match run() {
        Ok(answer) => answer,
        Err(error) => {
            tracing::warn!("答題層沒有成句，保留本機結果：{error}");
            None
        }
    }
}

#[tauri::command(async)]
fn ask(question: String, shell: tauri::State<'_, Shell>) -> Result<Answer, String> {
    use sister_core::question::{Intent, Shape};
    let question = question.trim().to_string();
    if question.is_empty() {
        return Ok(Answer {
            presentation_id: None,
            kind: "keywords",
            followup: None,
            closure_notice: None,
            searched: None,
            query_id: None,
            answers: Vec::new(),
            hits: Vec::new(),
            // 空字串不是「問了但沒找到」，是根本沒問。
            blind: None,
            truncated: false,
            answers_truncated: false,
            time_range: None,
            chapters: None,
            overview: None,
            synthesis: None,
            brain: BrainAnswer::not_configured(),
        });
    }
    // 每個非空問題都接管「最新題」槽。即使這一題隨後被全停擋住，也不能讓
    // 上一題的 provider 繼續在背景跑完。
    let answer_cli_claim = begin_answer_cli(&shell);
    // 問答不只是讀：closure、follow-up 與 query log 都可能寫 DB。整份 admission
    // 活到 renderer 同步畫完，讓 stop-all 能先發佈 Stopping、再等這一題收乾淨；
    // 全停後來的新題連 retrieval 都不進。
    let master_stop_admission = admit_desktop_brain(shell.data_dir.as_deref(), "這一題")?;
    let (brain, planned_searches) =
        plan_answer_searches(&shell, &question, answer_cli_claim.cancellation());
    // 題庫 latency 只量本機問答工作；CLI 查詢規劃已另記在
    // brain_outbound role=answer_search，不能把兩段時間混成一個數字。
    let started = std::time::Instant::now();

    // 沒有可用 CLI 時仍保留純本機的 L2 總覽；只要已選 CLI 且這題的查詢計畫
    // 完成，總覽問法也走同一條 CLI-directed retrieval，不再繞過大腦。
    if sister_core::question::intent(&question) == Intent::MemoryOverview
        && planned_searches.is_none()
    {
        let mut answer = with_db(&shell, |db| memory_overview_answer(db, started))?;
        answer.brain = brain;
        answer.presentation_id = Some(hold_presentation(master_stop_admission));
        return Ok(answer);
    }

    let retrieval_questions = planned_searches
        .as_ref()
        .map_or_else(|| vec![question.clone()], |planned| planned.queries.clone());
    let cli_directed = planned_searches.is_some();

    // 章節那一支要寫 `segment`，所以整條改拿可變借用。沒認到時間範圍
    // 時 `chapters_for_question` 立刻回 `None`，不會重算。
    let (mut answer, prepared) = with_db_mut(&shell, |db| {
        let now = sister_core::now_ms();
        let close = sister_core::reviewer::close_from_message(db, &question, now)
            .map_err(|e| format!("{e:#}"))?;
        let closure_notice = match close {
            sister_core::followup::CloseIntent::NotAClosure => None,
            sister_core::followup::CloseIntent::Unrecognized => {
                Some("我認不出你指哪一張記憶，所以沒有動任何一張。".to_string())
            }
            sister_core::followup::CloseIntent::Ambiguous { .. } => {
                Some("這句話對得上不只一張記憶，所以沒有動任何一張。".to_string())
            }
            sister_core::followup::CloseIntent::Close { .. } => {
                Some("這張記憶已結案，不會再提。".to_string())
            }
        };
        let previous = sister_core::reviewer::followup_state(db).map_err(|e| format!("{e:#}"))?;
        let followup = match sister_core::followup::decide(
            &db.live_commitments().map_err(|e| format!("{e:#}"))?,
            now,
            previous.as_ref(),
        ) {
            sister_core::followup::FollowupDecision::Ask {
                commitment_id,
                text,
            } => {
                sister_core::reviewer::record_followup(db, commitment_id, now)
                    .map_err(|e| format!("{e:#}"))?;
                Some(text)
            }
            sister_core::followup::FollowupDecision::NoEligibleCommitment
            | sister_core::followup::FollowupDecision::CoolingDown { .. } => None,
        };
        // 每條都是已選 CLI 要求的自然語言查詢，由 AI-Sister 在本機執行。多條
        // 查詢會依 CLI 給的順序合併並去重，最後仍守原本 facts 10／原文 20 的上限。
        const FACTS: usize = 10;
        const HITS: usize = 20;
        let mut shape = Shape::Keywords;
        let mut facts = Vec::new();
        let mut hits = Vec::new();
        let mut fact_ids = HashSet::new();
        let mut chunk_ids = HashSet::new();
        let mut facts_truncated = false;
        let mut truncated = false;
        for (index, retrieval_question) in retrieval_questions.iter().enumerate() {
            let retrieval = sister_core::retrieval::RetrievalProfile::TextAndFacts
                .retrieve_with_limits(
                    db,
                    retrieval_question,
                    sister_core::retrieval::RetrievalLimits::new(FACTS, HITS),
                )
                .map_err(|e| format!("{e:#}"))?;
            if index == 0 {
                shape = retrieval.shape;
            }
            facts_truncated |= retrieval.answers_truncated;
            truncated |= retrieval.hits_truncated;
            facts.extend(
                retrieval
                    .answers
                    .into_iter()
                    .filter(|answer| fact_ids.insert(answer.latest.id)),
            );
            hits.extend(
                retrieval
                    .hits
                    .into_iter()
                    .filter(|hit| chunk_ids.insert(hit.chunk_id)),
            );
        }
        facts_truncated |= facts.len() > FACTS;
        truncated |= hits.len() > HITS;
        facts.truncate(FACTS);
        hits.truncate(HITS);
        let asked_chapters = if retrieval_questions.len() == 1 {
            db.chapters_for_question(&retrieval_questions[0], sister_core::now_ms())
                .map_err(|e| format!("{e:#}"))?
        } else {
            None
        };
        let prepared =
            sister_core::grounded_answer::prepare(&question, &facts, &hits, sister_core::now_ms())
                .map_err(|e| format!("{e:#}"))?;
        // **他打的那句話不進記錄檔。** 只留形狀、幾筆、幾毫秒——這三個數字
        // 足以回答「她是不是又卡住了」，而問題本身是他的東西，不是我的。
        tracing::info!(
            "問了一次（{}）：{} 個答案、{} 筆原文，{} ms",
            if shape == Shape::Recent || shape == Shape::Range {
                "時間"
            } else {
                "關鍵字"
            },
            facts.len(),
            hits.len(),
            started.elapsed().as_millis()
        );
        // 進題庫。他打的原話在**資料庫**裡，不在記錄檔裡——記錄檔是我會看的
        // 東西，資料庫是他的。刪得掉（時間軸上那條「忘掉這一段」會一起帶走）、
        // 過得了期（跟著文字的保留期）。理由與代價寫在 DATA_INVENTORY。
        //
        // 記不進去不算失敗：他要的是答案。
        //
        // 每次都重讀設定檔，不快取：他剛在設定頁上把那個勾拿掉，下一個問題就
        // 不該再被記。和暫停控制狀態同一條紀律——真相在磁碟上，這個行程只是鏡子。
        // 讀不到設定檔就當成不要記（`unwrap_or(false)`）：不確定的時候少存
        // 一點，方向和其他每一個 fail-closed 一致。
        let wanted = config_path()
            .and_then(|p| sister_core::config::Config::load(&p).map_err(|e| format!("{e:#}")))
            .map(|c| c.privacy.query_log)
            .unwrap_or(false);
        let query_id = wanted
            .then(|| {
                db.log_query(&sister_core::db::QueryLogEntry {
                    ts: sister_core::now_ms(),
                    question: &question,
                    shape: shape.name(),
                    // ★ 答案也算——她給了他東西就不是「答不出來」。
                    // 見 `QueryLogEntry::hits`。
                    hits: facts.len() + hits.len(),
                    latency_ms: started.elapsed().as_millis() as i64,
                    source: sister_core::db::SOURCE_DESKTOP,
                })
                .map_err(|e| tracing::warn!("這一題沒記進題庫：{e}"))
                .ok()
            })
            .flatten();
        // 只有兩手空空的時候才去問。有答案的話這幾個 COUNT 是白跑的，而這條
        // 路上使用者正等著看畫面。
        let blind = if facts.is_empty() && hits.is_empty() {
            // 比對用的是 `terms`，掃描界線也照 `terms` 判——理由和
            // `sister query` 那邊同一條。
            let asked = sister_core::question::terms(&retrieval_questions[0]);
            // 不給空路徑當退路：`pause::is_paused` 的規矩是「問不出來就當成
            // 暫停」，而 `Path::new("")` 會讓它去工作目錄找一個不存在的旗標、
            // 然後回一個很有把握的「沒有暫停」。寧可這一段沒有理由可講。
            let dir = shell
                .data_dir
                .as_deref()
                .ok_or_else(|| "找不到資料目錄".to_string())?;
            let b =
                sister_core::answer::blind_spots(db, dir, asked).map_err(|e| format!("{e:#}"))?;
            Some(Blind::from(b))
        } else {
            None
        };
        // 出處的 `frame_id` 一路上都只回答「這段字抄自哪一幀」——那是來源，
        // 一直都在。畫面上那個「點開看當時的畫面」問的卻是另一件事：那一幀
        // 有沒有留下照片。整份答案畫完問一次，沒有照片的就把鑰匙收回去。
        let openable = {
            let ids: Vec<i64> = hits
                .iter()
                .filter_map(|h| h.frame_id)
                .chain(facts.iter().filter_map(|a| a.latest.frame_id))
                .collect();
            db.frames_with_image(&ids).map_err(|e| format!("{e:#}"))?
        };
        let answer = Answer {
            presentation_id: None,
            kind: shape.name(),
            followup,
            closure_notice,
            // 只在**不一樣**的時候送。一樣的時候送過去，畫面那邊還要再比一次，
            // 而「這兩串字算不算同一句」是這裡才知道的事（`terms` 回的是原句的
            // 一個切片）。`Shape::Recent` 根本沒走比對那條路，所以也不送。
            searched: match shape {
                Shape::Recent | Shape::Range => None,
                Shape::Keywords if !cli_directed => {
                    // 只在**黏過**的時候送。剝掉「剛剛那個」留下「優惠方案」是
                    // 剝對了，每次都報一句只會讓人學會忽略它；黏出「個板」才是
                    // 她找了一個不是詞的東西。
                    let (t, glued) = sister_core::question::terms_with_retreat(&question);
                    glued.then(|| t.to_string())
                }
                // CLI 已經改寫過查詢時，原問句的 `terms` 不再是實際拿去比對的字；
                // 不能把它畫成這一輪的搜尋真相。
                Shape::Keywords => None,
            },
            query_id,
            blind,
            truncated,
            answers_truncated: facts_truncated,
            answers: facts
                .into_iter()
                .map(|a| Fact {
                    fact_id: a.latest.id,
                    value: a.latest.normalized,
                    raw: a.latest.raw,
                    sightings: a.sightings,
                    ts: a.latest.ts,
                    chunk_id: a.latest.chunk_id,
                    frame_id: a.latest.frame_id.filter(|id| openable.contains(id)),
                    app: a.latest.app_id,
                    title: a.latest.window_title,
                    url: a.latest.url,
                })
                .collect(),
            hits: hits
                .into_iter()
                .map(|h| Hit {
                    chunk_id: h.chunk_id,
                    ts: h.ts,
                    text: h.text,
                    snippet: h.snippet,
                    app: h.app_id,
                    title: h.window_title,
                    url: h.url,
                    frame_id: h.frame_id.filter(|id| openable.contains(id)),
                })
                .collect(),
            time_range: asked_chapters.as_ref().map(|(r, _)| AskedTimeRange {
                from: r.from,
                to: r.to,
                said: r.said.clone(),
            }),
            chapters: asked_chapters
                .map(|(_, ch)| ch.into_iter().map(chapter_from_activity).collect()),
            overview: None,
            synthesis: None,
            brain,
        };
        Ok((answer, prepared))
    })?;
    if let Some(planned) = planned_searches {
        if let Some(prepared) = prepared {
            answer.synthesis = synthesize_grounded_answer(
                &shell,
                &answer,
                &prepared,
                &planned.cli,
                answer_cli_claim.cancellation(),
                &master_stop_admission,
            );
            answer.brain.state = if answer.synthesis.is_some() {
                "used"
            } else {
                "answer_failed"
            };
        } else {
            answer.brain.state = "no_sources";
        }
    }
    answer.presentation_id = Some(hold_presentation(master_stop_admission));
    Ok(answer)
}

/// 時間軸上的一天。
#[derive(Serialize)]
struct Day {
    start_ts: i64,
    chunks: i64,
    first_ts: i64,
    last_ts: i64,
}

/// 哪幾天她其實有在看。
///
/// `tz_offset_ms` 由視窗傳進來（`-new Date().getTimezoneOffset() * 60000`），
/// 不是在 Rust 這邊算的：core 刻意不認識時區，而畫面本來就要用同一個偏移量把
/// 日期印出來——兩邊各算一次遲早會在日光節約時間那天對不起來。
///
/// 收到之後仍然夾一次。合法範圍是 UTC−12 到 UTC+14，超出去的值只可能來自
/// 前端的 bug，而一個離譜的偏移量會把「一天」切在莫名其妙的地方，然後看起來
/// 只是資料很怪。
#[tauri::command(async)]
fn timeline_days(tz_offset_ms: i64, shell: tauri::State<'_, Shell>) -> Result<Vec<Day>, String> {
    const H: i64 = 3_600_000;
    let tz = tz_offset_ms.clamp(-12 * H, 14 * H);
    with_db(&shell, |db| {
        Ok(db
            .days_with_data(tz)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|d| Day {
                start_ts: d.start_ts,
                chunks: d.chunks,
                first_ts: d.first_ts,
                last_ts: d.last_ts,
            })
            .collect())
    })
}

/// 時間軸上的一格。
#[derive(Serialize)]
struct Moment {
    ts: i64,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
    text: String,
    /// `None` = 字還在，但沒有畫面可以給他看。同 [`Hit::frame_id`]。
    frame_id: Option<i64>,
}

/// 她閉眼的一段。兩端都可以是 `None`，見 [`sister_core::db::PauseSpan`]。
#[derive(Serialize)]
struct Gap {
    from: Option<i64>,
    to: Option<i64>,
}

/// 一天的內容。
#[derive(Serialize)]
struct DayView {
    moments: Vec<Moment>,
    /// 這一天她被關掉的那幾段。**沒有這個欄位的時間軸會說謊**：一片空白到底
    /// 是他去開會了，還是她被按了暫停，在畫面上長得一模一樣。
    pauses: Vec<Gap>,
    /// 這一天還有更多沒送過來。安靜地截斷會讓一整天看起來比實際短。
    truncated: bool,
}

/// 一天裡她看到的東西。
#[tauri::command(async)]
fn timeline_moments(
    from_ts: i64,
    to_ts: i64,
    limit: usize,
    shell: tauri::State<'_, Shell>,
) -> Result<DayView, String> {
    // 上限再夾一次：前端要多少就給多少的話，一個算錯的日期範圍會把一整年
    // 的文字塞進 webview，然後那個視窗就沒了。
    let limit = limit.clamp(1, 2_000);
    with_db(&shell, |db| {
        // 多要一筆，用來判斷「還有沒有」。少了這一步就只能猜——而猜錯的方向
        // 是「剛好滿 limit 筆」被當成剛好結束。
        let mut rows = db
            .timeline(from_ts, to_ts, limit + 1)
            .map_err(|e| format!("{e:#}"))?;
        let truncated = rows.len() > limit;
        rows.truncate(limit);

        let pauses = db
            .pause_spans(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|s| Gap {
                from: s.from,
                to: s.to,
            })
            .collect();

        // 同 `ask`：來源一直都在，照片不一定。點得開的才給鑰匙。
        let openable = {
            let ids: Vec<i64> = rows.iter().filter_map(|m| m.frame_id).collect();
            db.frames_with_image(&ids).map_err(|e| format!("{e:#}"))?
        };
        Ok(DayView {
            moments: rows
                .into_iter()
                .map(|m| Moment {
                    ts: m.ts,
                    app: m.app,
                    title: m.title,
                    url: m.url,
                    text: m.text,
                    frame_id: m.frame_id.filter(|id| openable.contains(id)),
                })
                .collect(),
            pauses,
            truncated,
        })
    })
}

/// 時間軸上的一段。時間是含 5 秒重疊 margin 的顯示範圍。
#[derive(Serialize)]
struct Chapter {
    start_ts: i64,
    end_ts: i64,
    core_start_ts: i64,
    core_end_ts: i64,
    app: Option<String>,
    title: Option<String>,
    host: Option<String>,
    /// 打開這一段的切刀。空 = 當天第一段，沒有打開它的切刀。
    cut_kinds: Vec<String>,
    /// 沒有邊界可算是 `None`，不是 0.0。
    confidence: Option<f32>,
    /// 使用者編輯留下的形狀。演算法自己切的是 `None`。
    edited: Option<String>,
    /// 套用過的那一筆 `segment_edit.id`。沒有就是 `None`。
    edit_id: Option<i64>,
    /// 由幾個分鐘級 segment 併成。時間軸上的一格就是一段，不填。
    #[serde(skip_serializing_if = "Option::is_none")]
    segment_count: Option<usize>,
    /// 核心時長。答案用這個。時間軸不填——那裡的 start_ts／end_ts 含 5 秒 margin。
    #[serde(skip_serializing_if = "Option::is_none")]
    core_ms: Option<i64>,
    /// 底下的分鐘級段落。活動級才填；答案端不送。
    #[serde(skip_serializing_if = "Option::is_none")]
    segments: Option<Vec<Chapter>>,
    /// 這一段上最新的 L2 假設。沒有就是沒有，不是空物件。
    #[serde(skip_serializing_if = "Option::is_none")]
    l2: Option<Vec<sister_core::brain::L2View>>,
}

fn chapter_from_segment(s: sister_core::segment::Segment) -> Chapter {
    Chapter {
        start_ts: s.started_at,
        end_ts: s.ended_at,
        core_start_ts: s.core_started_at,
        core_end_ts: s.core_ended_at,
        app: s.app,
        title: s.title,
        host: s.host,
        cut_kinds: s.cut_kinds.iter().map(|k| k.as_str().to_string()).collect(),
        confidence: s.confidence,
        edited: s.last_edit.map(|e| e.kind.as_str().to_string()),
        edit_id: s.last_edit.map(|e| e.id),
        segment_count: None,
        core_ms: None,
        segments: None,
        l2: None,
    }
}

/// 答案端的一格。鐘面和時長都用核心時間；`segment_count` 說它是由幾段併成的。
fn chapter_from_activity(a: sister_core::activity::Activity) -> Chapter {
    chapter_from_activity_with_nested(a, false)
}

/// 時間軸上的一格。顯示範圍含 5 秒 margin（好把 moments 裝進去），時長仍用核心。
fn chapter_from_activity_timeline(a: sister_core::activity::Activity) -> Chapter {
    chapter_from_activity_with_nested(a, true)
}

fn chapter_from_activity_with_nested(a: sister_core::activity::Activity, nested: bool) -> Chapter {
    let core_ms = a.core_ms();
    let last = a.last_edit();
    let opening = a
        .segments
        .first()
        .map(|s| s.cut_kinds.iter().map(|k| k.as_str().to_string()).collect())
        .unwrap_or_default();
    let confidence = a.segments.first().and_then(|s| s.confidence);
    let (start_ts, end_ts) = if nested {
        (a.started_at, a.ended_at)
    } else {
        (a.core_started_at, a.core_ended_at)
    };
    let segments = nested.then(|| a.segments.into_iter().map(chapter_from_segment).collect());
    Chapter {
        start_ts,
        end_ts,
        core_start_ts: a.core_started_at,
        core_end_ts: a.core_ended_at,
        app: a.app,
        title: a.title,
        host: a.host,
        cut_kinds: opening,
        confidence,
        edited: last.map(|e| e.kind.as_str().to_string()),
        edit_id: last.map(|e| e.id),
        segment_count: Some(a.segment_count),
        core_ms: Some(core_ms),
        segments,
        l2: None,
    }
}

/// 某一天切成的段落。打開時間軸才算，不在錄製那條路上。
#[tauri::command(async)]
fn timeline_chapters(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let cards = db
            .l2_in_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        Ok(db
            .activities_for_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|a| {
                let mut ch = chapter_from_activity_timeline(a);
                attach_l2(&mut ch, &cards);
                ch
            })
            .collect())
    })
}

fn attach_l2(ch: &mut Chapter, cards: &[sister_core::db::L2CardRow]) {
    let views = sister_core::brain::chapter_l2_views(cards, ch.core_start_ts, ch.core_end_ts);
    ch.l2 = if views.is_empty() { None } else { Some(views) };
}

fn timeline_chapters_after_edit(
    segs: Vec<sister_core::segment::Segment>,
    cards: &[sister_core::db::L2CardRow],
) -> Vec<Chapter> {
    sister_core::activity::group(&segs)
        .into_iter()
        .map(|a| {
            let mut ch = chapter_from_activity_timeline(a);
            attach_l2(&mut ch, cards);
            ch
        })
        .collect()
}

/// 把相鄰兩段併成一段。立刻回新的章節清單，不用重開視窗。
///
/// `left_core_start`／`right_core_start` 仍是分鐘級 segment 的核心起點。
/// 活動級畫面上「與下一段合併」要傳左件最後一段、右件第一段。
#[tauri::command(async)]
fn timeline_merge_chapters(
    left_core_start: i64,
    right_core_start: i64,
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let segs = db
            .merge_chapters(left_core_start, right_core_start, from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let cards = db
            .l2_in_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        Ok(timeline_chapters_after_edit(segs, &cards))
    })
}

/// 在 `at_ts` 把一段切成兩段。
#[tauri::command(async)]
fn timeline_split_chapter(
    at_ts: i64,
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let segs = db
            .split_chapter(at_ts, from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let cards = db
            .l2_in_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        Ok(timeline_chapters_after_edit(segs, &cards))
    })
}

/// 撤銷某一筆合併或切開。多寫一列，不改舊的訓練訊號。
#[tauri::command(async)]
fn timeline_undo_segment_edit(
    edit_id: i64,
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<Chapter>, String> {
    with_db_mut(&shell, |db| {
        let segs = db
            .undo_segment_edit(edit_id, from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let cards = db
            .l2_in_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        Ok(timeline_chapters_after_edit(segs, &cards))
    })
}

#[tauri::command(async)]
fn memory_guesses(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Vec<sister_core::brain::L2View>, String> {
    with_db(&shell, |db| {
        let cards = db
            .l2_in_range(from_ts, to_ts)
            .map_err(|e| format!("{e:#}"))?;
        let mut by_seg: std::collections::BTreeMap<i64, Vec<&sister_core::db::L2CardRow>> =
            std::collections::BTreeMap::new();
        for c in &cards {
            by_seg.entry(c.segment_core_start).or_default().push(c);
        }
        let mut views = Vec::new();
        for versions in by_seg.values() {
            let mut ordered = versions.clone();
            ordered.sort_by_key(|c| (c.version, c.id));
            if let Some((row, prev)) = sister_core::brain::latest_with_previous(&ordered) {
                let row = *row;
                let prev = prev.copied();
                views.push(sister_core::brain::view_from_row_with_previous(row, prev));
            }
        }
        Ok(views)
    })
}

#[derive(Serialize)]
struct CurrentGuessView {
    status: sister_core::brain::CurrentGuess,
    message: String,
    card: Option<sister_core::brain::L2View>,
}

/// 「現在」只在這裡組資料；狀態分類與人看到的字都由 sister-core 決定。
#[tauri::command(async)]
fn memory_current_guess(shell: tauri::State<'_, Shell>) -> Result<CurrentGuessView, String> {
    let dir = shell
        .data_dir
        .as_deref()
        .ok_or_else(|| "找不到資料目錄，不能判斷現在的錄製狀態".to_string())?;
    let presence = sister_core::heartbeat::presence(dir, sister_core::now_ms());
    let paused = sister_core::pause::is_paused(dir);
    let mut card = None;
    let status = sister_core::brain::CurrentGuess::decide(presence, paused, || {
        let config =
            sister_core::config::Config::load(&config_path()?).map_err(|e| format!("{e:#}"))?;
        let consented = sister_core::consent::load(dir).cloud_permit().is_some();
        let now = sister_core::now_ms();
        with_db_mut(&shell, |db| {
            let from = db
                .current_session_started_at()
                .map_err(|e| format!("{e:#}"))?
                .unwrap_or(now);
            let mut segments = db
                .chapters_for_range(from, now)
                .map_err(|e| format!("{e:#}"))?;
            // 錄製中最後一段還開著，解釋層不送它：`wakeup.rs` 的 `include_open ==
            // false` 那條路把右界設成 `segs.last().core_started_at`，而
            // `chapters_for_range` 過濾 `core_started_at < to_ts`，所以最後一段剛好
            // 被排除在外。這裡照同一條線拿掉它，再看最新的已關閉段落——不然畫面會
            // 對一段永遠不會有卡的段落說「還在排隊」。
            segments.pop();
            let latest = segments.pop();
            let Some(seg) = latest else {
                return Ok(sister_core::brain::RecordingFacts {
                    latest_closed: None,
                    has_command: config.brain.cli().is_some(),
                    consented,
                    used_today: 0,
                    daily_budget: config.brain.daily_budget,
                    previous_attempts: None,
                });
            };
            let versions = db
                .l2_versions_for_chapter(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?;
            card = sister_core::brain::latest_with_previous(&versions).map(|(row, previous)| {
                sister_core::brain::view_from_row_with_previous(row, previous)
            });
            let facts = db
                .facts_in_range(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?;
            let large_clip = db
                .clipboard_in_range(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .any(|c| c.byte_len >= sister_core::segment::LARGE_CLIPBOARD_BYTES);
            let stuck = db
                .stuck_in_range(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .any(|s| s.started_at < seg.core_ended_at && s.ended_at > seg.core_started_at);
            let worth = sister_core::brain::worth_interpreting(&seg, &facts, large_clip, stuck);
            let day = sister_core::brain::local_day_key(now)
                .ok_or_else(|| "算不出今天的日期，不能核對解釋預算".to_string())?;
            let used = db
                .brain_outbound_count_on(&day)
                .map_err(|e| format!("{e:#}"))?;
            // 這一格的查詢一律把錯誤往上帶，整塊算完才交出去；任何一個查詢失敗，
            // 包括上面已經算好的 card，都會讓整塊失敗。brain 外送已經發生後的輔助查詢
            // 則不能擋住那次外送，所以 brain.rs 那邊會用 `.ok().flatten()`。
            let previous_attempts = db
                .retained_interpreter_attempts_for_segment(seg.core_started_at, seg.core_ended_at)
                .map_err(|e| format!("{e:#}"))?;
            let latest_closed = sister_core::brain::LatestClosedSegment {
                has_card: card.is_some(),
                worth_interpreting: worth,
            };
            Ok(sister_core::brain::RecordingFacts {
                latest_closed: Some(latest_closed),
                has_command: config.brain.cli().is_some(),
                consented,
                used_today: used,
                daily_budget: config.brain.daily_budget,
                previous_attempts,
            })
        })
    })?;
    Ok(CurrentGuessView {
        message: status.message(),
        status,
        card,
    })
}

#[derive(Serialize)]
struct PledgeView {
    id: i64,
    text: String,
    kind: String,
    status: String,
    due_hint: Option<String>,
    due_source: Option<String>,
    confidence: f64,
    tombstoned: bool,
    kill_note: Option<String>,
    evidence: Vec<sister_core::brain::L2EvidenceView>,
}

#[tauri::command(async)]
fn memory_commitments(shell: tauri::State<'_, Shell>) -> Result<Vec<PledgeView>, String> {
    with_db(&shell, |db| {
        let rows = db.all_commitments().map_err(|e| format!("{e:#}"))?;
        Ok(rows
            .into_iter()
            .map(|c| {
                let refs: Vec<String> = serde_json::from_str(&c.evidence_json).unwrap_or_default();
                PledgeView {
                    id: c.id,
                    text: c.text,
                    kind: c.kind,
                    status: c.status,
                    due_hint: c.due_hint,
                    due_source: c.due_source,
                    confidence: c.confidence,
                    tombstoned: c.tombstoned_at.is_some(),
                    kill_note: c.kill_note,
                    evidence: refs
                        .iter()
                        .filter_map(|s| sister_core::brain::EvidenceRef::parse(s))
                        .map(|r| match r {
                            sister_core::brain::EvidenceRef::Frame(id) => {
                                sister_core::brain::L2EvidenceView {
                                    kind: "frame",
                                    id,
                                    label: format!("畫面 #{id}"),
                                }
                            }
                            sister_core::brain::EvidenceRef::Fact(id) => {
                                sister_core::brain::L2EvidenceView {
                                    kind: "fact",
                                    id,
                                    label: format!("本機事實 #{id}"),
                                }
                            }
                        })
                        .collect(),
                }
            })
            .collect())
    })
}

#[derive(Serialize)]
struct OutboundLine {
    ts: i64,
    command: String,
    args: Vec<String>,
    chars_sent: i64,
    truncated: bool,
    outcome: String,
    duration_ms: i64,
    error: Option<String>,
    role: String,
}

#[derive(Serialize)]
struct SkipLine {
    ts: i64,
    reason: String,
    detail: String,
}

#[derive(Serialize)]
struct OutboundLog {
    outbound: Vec<OutboundLine>,
    skips: Vec<SkipLine>,
    ever_sent: bool,
}

#[tauri::command(async)]
fn memory_outbound(
    limit: Option<u32>,
    shell: tauri::State<'_, Shell>,
) -> Result<OutboundLog, String> {
    let take = limit.unwrap_or(200).clamp(1, 500) as usize;
    with_db(&shell, |db| {
        let outbound = db
            .list_brain_outbound(take)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|row| OutboundLine {
                ts: row.ts,
                command: row.command,
                args: serde_json::from_str(&row.args_json).unwrap_or_default(),
                chars_sent: row.chars_sent,
                truncated: row.truncated,
                outcome: row.outcome,
                duration_ms: row.duration_ms,
                error: row.error,
                role: row.role,
            })
            .collect();
        let skips = db
            .list_brain_skip(take)
            .map_err(|e| format!("{e:#}"))?
            .into_iter()
            .map(|row| SkipLine {
                ts: row.ts,
                reason: row.reason,
                detail: row.detail,
            })
            .collect();
        let ever_sent = db.ever_brain_outbound().map_err(|e| format!("{e:#}"))?;
        Ok(OutboundLog {
            outbound,
            skips,
            ever_sent,
        })
    })
}

/// 這一天的日摘要。三種「沒有」是三個 `kind`，不是同一個空物件。
#[tauri::command(async)]
fn memory_day_summary(
    from_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<sister_core::db::DaySummaryGlance, String> {
    let date = sister_core::brain::local_day_key(from_ts)
        .ok_or_else(|| "算不出這一天的日期".to_string())?;
    with_db(&shell, |db| {
        db.day_summary_glance(&date).map_err(|e| format!("{e:#}"))
    })
}

#[tauri::command(async)]
fn correct_l2(
    segment_core_start: i64,
    activity: String,
    shell: tauri::State<'_, Shell>,
) -> Result<(), String> {
    with_db_mut(&shell, |db| {
        sister_core::reviewer::correct_l2(db, segment_core_start, &activity)
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
    })
}

#[tauri::command(async)]
fn commitment_kill(
    id: i64,
    note: Option<String>,
    shell: tauri::State<'_, Shell>,
) -> Result<(), String> {
    with_db_mut(&shell, |db| {
        sister_core::reviewer::kill_commitment(
            db,
            id,
            note.as_deref().unwrap_or("使用者結案"),
            sister_core::now_ms(),
        )
        .map_err(|e| format!("{e:#}"))?;
        Ok(())
    })
}

#[tauri::command(async)]
fn commitment_other(id: i64, shell: tauri::State<'_, Shell>) -> Result<(), String> {
    with_db_mut(&shell, |db| {
        sister_core::reviewer::snooze_commitment(db, id, sister_core::now_ms())
            .map_err(|e| format!("{e:#}"))?;
        Ok(())
    })
}

/// Persona 素材包在畫面上的狀態。
///
/// 每個狀態都描述 native authority 對 exact cache 的判斷；它不掃任意本機目錄，
/// 更不會把「看見一個檔案」當成「權利與 digest 都驗過」。
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PersonaAssetPackPhase {
    Unavailable,
    Available,
    Installing,
    Removing,
    RepairNeeded,
    Installed,
}

/// 舊 pack 中只有已驗證的 portrait 才會出現在這個相容欄位。renderer 的 17 張
/// current portrait 已隨程式提供，不讀這格，也不會因 pack 狀態改成字母。
#[derive(Clone, Serialize)]
struct PersonaPortraitView {
    data_url: String,
}

/// 公開 manifest 裡可由 avatar click 指到的八條無條件 fixed voice。封閉 enum 讓
/// renderer 不能把任意路徑或私人文字塞進 IPC。pack 另有需要活動證據的 active clips，
/// 但 click rotation 沒有那份證據，所以不把它們放進這個 enum。
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PersonaVoiceLineId {
    ChatgptGreeting,
    ChatgptQuiet,
    ClaudeGreeting,
    ClaudeQuiet,
    GeminiGreeting,
    GeminiQuiet,
    GrokGreeting,
    GrokQuiet,
}

impl PersonaVoiceLineId {
    fn persona(self) -> sister_core::config::PersonaId {
        use sister_core::config::PersonaId;
        match self {
            Self::ChatgptGreeting | Self::ChatgptQuiet => PersonaId::Chatgpt,
            Self::ClaudeGreeting | Self::ClaudeQuiet => PersonaId::Claude,
            Self::GeminiGreeting | Self::GeminiQuiet => PersonaId::Gemini,
            Self::GrokGreeting | Self::GrokQuiet => PersonaId::Grok,
        }
    }

    fn asset_line(self) -> sister_assets::VoiceLine {
        use sister_assets::VoiceLine;
        match self {
            Self::ChatgptGreeting => VoiceLine::ChatgptGreeting,
            Self::ChatgptQuiet => VoiceLine::ChatgptQuiet,
            Self::ClaudeGreeting => VoiceLine::ClaudeGreeting,
            Self::ClaudeQuiet => VoiceLine::ClaudeQuiet,
            Self::GeminiGreeting => VoiceLine::GeminiGreeting,
            Self::GeminiQuiet => VoiceLine::GeminiQuiet,
            Self::GrokGreeting => VoiceLine::GrokGreeting,
            Self::GrokQuiet => VoiceLine::GrokQuiet,
        }
    }

    fn from_asset(line: sister_assets::VoiceLine) -> Self {
        use sister_assets::VoiceLine;
        match line {
            VoiceLine::ChatgptGreeting => Self::ChatgptGreeting,
            VoiceLine::ChatgptQuiet => Self::ChatgptQuiet,
            VoiceLine::ClaudeGreeting => Self::ClaudeGreeting,
            VoiceLine::ClaudeQuiet => Self::ClaudeQuiet,
            VoiceLine::GeminiGreeting => Self::GeminiGreeting,
            VoiceLine::GeminiQuiet => Self::GeminiQuiet,
            VoiceLine::GrokGreeting => Self::GrokGreeting,
            VoiceLine::GrokQuiet => Self::GrokQuiet,
        }
    }
}

/// 開場只回 availability metadata，不把 WAV bytes 預先交給 renderer。真正的 bytes
/// 要等同一次 trusted avatar click 經 `persona_voice_read` 只取一條。
#[derive(Clone, Serialize)]
struct PersonaVoiceLineView {
    line_id: PersonaVoiceLineId,
    duration_ms: u32,
    spoken_text: &'static str,
}

#[derive(Serialize)]
struct PersonaVoicePayloadView {
    line_id: PersonaVoiceLineId,
    data_url: String,
}

#[derive(Clone, Serialize)]
struct PersonaAssetPackView {
    phase: PersonaAssetPackPhase,
    release_id: Option<String>,
    portrait: Option<PersonaPortraitView>,
    voice_lines: Vec<PersonaVoiceLineView>,
}

/// Persona cache 不跟 `sister --data-dir` 走。記憶 export／forget／prune 只處理
/// 記憶；這份可撤回的 public media 永遠住在 app 的 default data dir。
fn persona_asset_cache_root() -> Option<PathBuf> {
    sister_core::config::Config::default_data_dir()
        .map(|directory| directory.join(sister_assets::CACHE_DIRECTORY))
}

fn asset_persona(id: sister_core::config::PersonaId) -> Option<sister_assets::Persona> {
    use sister_core::config::PersonaId;
    Some(match id {
        PersonaId::Chatgpt => sister_assets::Persona::Chatgpt,
        PersonaId::Claude => sister_assets::Persona::Claude,
        PersonaId::Gemini => sister_assets::Persona::Gemini,
        PersonaId::Grok => sister_assets::Persona::Grok,
        PersonaId::Deepseek
        | PersonaId::Qwen
        | PersonaId::Mistral
        | PersonaId::Venice
        | PersonaId::Sakana
        | PersonaId::Perplexity
        | PersonaId::Glm
        | PersonaId::Kimi
        | PersonaId::Hunyuan
        | PersonaId::Minimax
        | PersonaId::Nemotron
        | PersonaId::Cohere
        | PersonaId::Mimo => return None,
    })
}

#[cfg(test)]
mod persona_asset_mapping_tests {
    use super::*;
    use sister_assets::{Persona as AssetPersona, VoiceLine};
    use sister_core::config::PersonaId;

    #[test]
    fn only_the_four_sisters_map_into_the_legacy_fixed_voice_pack() {
        assert_eq!(
            asset_persona(PersonaId::Chatgpt),
            Some(AssetPersona::Chatgpt)
        );
        assert_eq!(asset_persona(PersonaId::Claude), Some(AssetPersona::Claude));
        assert_eq!(asset_persona(PersonaId::Gemini), Some(AssetPersona::Gemini));
        assert_eq!(asset_persona(PersonaId::Grok), Some(AssetPersona::Grok));

        for id in [
            PersonaId::Deepseek,
            PersonaId::Qwen,
            PersonaId::Mistral,
            PersonaId::Venice,
            PersonaId::Sakana,
            PersonaId::Perplexity,
            PersonaId::Glm,
            PersonaId::Kimi,
            PersonaId::Hunyuan,
            PersonaId::Minimax,
            PersonaId::Nemotron,
            PersonaId::Cohere,
            PersonaId::Mimo,
        ] {
            assert_eq!(asset_persona(id), None, "{id:?} must stay on localService");
        }
    }

    #[test]
    fn every_fixed_voice_id_maps_to_the_same_persona_and_asset_line() {
        let cases = [
            (
                PersonaVoiceLineId::ChatgptGreeting,
                PersonaId::Chatgpt,
                VoiceLine::ChatgptGreeting,
            ),
            (
                PersonaVoiceLineId::ChatgptQuiet,
                PersonaId::Chatgpt,
                VoiceLine::ChatgptQuiet,
            ),
            (
                PersonaVoiceLineId::ClaudeGreeting,
                PersonaId::Claude,
                VoiceLine::ClaudeGreeting,
            ),
            (
                PersonaVoiceLineId::ClaudeQuiet,
                PersonaId::Claude,
                VoiceLine::ClaudeQuiet,
            ),
            (
                PersonaVoiceLineId::GeminiGreeting,
                PersonaId::Gemini,
                VoiceLine::GeminiGreeting,
            ),
            (
                PersonaVoiceLineId::GeminiQuiet,
                PersonaId::Gemini,
                VoiceLine::GeminiQuiet,
            ),
            (
                PersonaVoiceLineId::GrokGreeting,
                PersonaId::Grok,
                VoiceLine::GrokGreeting,
            ),
            (
                PersonaVoiceLineId::GrokQuiet,
                PersonaId::Grok,
                VoiceLine::GrokQuiet,
            ),
        ];
        for (view, persona, asset) in cases {
            assert!(view.persona() == persona);
            assert!(view.asset_line() == asset);
            assert!(PersonaVoiceLineId::from_asset(asset) == view);
            assert_eq!(
                asset.persona(),
                asset_persona(persona).expect("four sisters have pack IDs")
            );
        }
    }
}

fn local_asset_phase(shell: &Shell) -> Result<PersonaAssetPackPhase, String> {
    match shell.asset_operation.load(Ordering::Acquire) {
        ASSET_INSTALLING => return Ok(PersonaAssetPackPhase::Installing),
        ASSET_REMOVING => return Ok(PersonaAssetPackPhase::Removing),
        _ => {}
    }
    let Some(cache) = persona_asset_cache_root() else {
        return Ok(PersonaAssetPackPhase::Unavailable);
    };
    match sister_assets::cache_state(&cache) {
        Ok(sister_assets::CacheState::Available) => Ok(PersonaAssetPackPhase::Available),
        Ok(sister_assets::CacheState::RepairNeeded) => Ok(PersonaAssetPackPhase::RepairNeeded),
        Ok(sister_assets::CacheState::Installed) => Ok(PersonaAssetPackPhase::Installed),
        Err(error) => Err(format!("問不到 Persona 素材 cache 狀態：{error}")),
    }
}

/// fixed pack 的唯一本機 resolver seam。它只從 embedded authority 指到的 cache
/// 讀 bytes；renderer 不能傳路徑，也不會沿這條路觸發下載。
fn resolve_local_persona_assets(
    shell: &Shell,
    persona: sister_core::config::PersonaId,
) -> PersonaAssetPackView {
    // 主畫面不能因選配素材的 lock／I/O 問題連角色設定都讀不到。設定頁的獨立
    // status command 會保留完整錯誤；這裡只做 fail-closed presentation fallback。
    let phase = local_asset_phase(shell).unwrap_or(PersonaAssetPackPhase::Unavailable);
    if phase != PersonaAssetPackPhase::Installed {
        return PersonaAssetPackView {
            phase,
            release_id: None,
            portrait: None,
            voice_lines: Vec::new(),
        };
    }
    let Some(cache) = persona_asset_cache_root() else {
        return PersonaAssetPackView {
            phase: PersonaAssetPackPhase::Unavailable,
            release_id: None,
            portrait: None,
            voice_lines: Vec::new(),
        };
    };
    let Some(persona) = asset_persona(persona) else {
        return PersonaAssetPackView {
            phase,
            release_id: Some(sister_assets::RELEASE_ID.to_string()),
            portrait: None,
            voice_lines: Vec::new(),
        };
    };

    let resolved = sister_assets::read_portrait(&cache, persona).and_then(|portrait| {
        let voice_lines = sister_assets::voice_metadata(&cache, persona)?
            .into_iter()
            .map(|voice| PersonaVoiceLineView {
                line_id: PersonaVoiceLineId::from_asset(voice.line),
                duration_ms: voice.duration_ms,
                spoken_text: voice.spoken_text,
            })
            .collect();
        Ok((portrait, voice_lines))
    });
    match resolved {
        Ok((portrait, voice_lines)) => PersonaAssetPackView {
            phase,
            release_id: Some(sister_assets::RELEASE_ID.to_string()),
            portrait: Some(PersonaPortraitView {
                data_url: format!(
                    "data:image/webp;base64,{}",
                    sister_shell::base64(&portrait.bytes)
                ),
            }),
            voice_lines,
        },
        Err(_) => PersonaAssetPackView {
            phase: PersonaAssetPackPhase::RepairNeeded,
            release_id: None,
            portrait: None,
            voice_lines: Vec::new(),
        },
    }
}

/// 單條 fixed voice 讀取。line 是封閉 enum，cache path 與檔名完全由 embedded
/// authority 決定；私人文字、renderer 路徑或遠端 URL 都進不了這個簽章。
fn resolve_local_persona_voice(line_id: PersonaVoiceLineId) -> Option<PersonaVoicePayloadView> {
    let cache = persona_asset_cache_root()?;
    let voice = sister_assets::read_voice(&cache, line_id.asset_line()).ok()?;
    Some(PersonaVoicePayloadView {
        line_id,
        data_url: format!(
            "data:audio/wav;base64,{}",
            sister_shell::base64(&voice.bytes)
        ),
    })
}

#[derive(Clone, Serialize)]
struct PersonaView {
    enabled: sister_core::config::PersonaVisible,
    id: sister_core::config::PersonaId,
    motion: sister_core::config::PersonaMotionEnabled,
    tap_lines: sister_core::config::PersonaTapLinesEnabled,
    /// 出廠永遠 false；設定頁另行明確打開以前，素材包存在也不能越過它。
    voice_enabled: sister_core::config::PersonaVoiceEnabled,
    asset_pack: PersonaAssetPackView,
}

fn persona_view(config: &sister_core::config::Config, shell: &Shell) -> PersonaView {
    let persona = config.shell.persona;
    PersonaView {
        enabled: persona.visible(),
        id: persona.id,
        motion: persona.motion_enabled(),
        tap_lines: persona.tap_lines_enabled(),
        voice_enabled: persona.voice_enabled(),
        asset_pack: resolve_local_persona_assets(shell, persona.id),
    }
}

#[tauri::command(async)]
fn persona_read(shell: tauri::State<'_, Shell>) -> Result<PersonaView, String> {
    let config =
        sister_core::config::Config::load(&config_path()?).map_err(|e| format!("{e:#}"))?;
    Ok(persona_view(&config, &shell))
}

#[tauri::command(async)]
fn persona_voice_read(
    line_id: PersonaVoiceLineId,
    shell: tauri::State<'_, Shell>,
) -> Result<Option<PersonaVoicePayloadView>, String> {
    let config =
        sister_core::config::Config::load(&config_path()?).map_err(|e| format!("{e:#}"))?;
    let persona = config.shell.persona;
    if !persona.visible().get()
        || !persona.tap_lines_enabled().get()
        || !persona.voice_enabled().get()
        || persona.id != line_id.persona()
    {
        return Ok(None);
    }

    let assets = resolve_local_persona_assets(&shell, persona.id);
    if assets.phase != PersonaAssetPackPhase::Installed
        || !assets
            .voice_lines
            .iter()
            .any(|line| line.line_id == line_id)
    {
        return Ok(None);
    }
    Ok(resolve_local_persona_voice(line_id))
}

#[derive(Clone, Serialize)]
struct PersonaAssetDisclosureView {
    release_id: &'static str,
    host: &'static str,
    path: &'static str,
    bytes: usize,
    boundary: &'static str,
}

#[derive(Clone, Serialize)]
struct PersonaAssetManagerView {
    phase: PersonaAssetPackPhase,
    disclosure: PersonaAssetDisclosureView,
    /// `None` 是尚未有一份完整安裝，不用 0 冒充「裝了 0 bytes」。
    asset_file_bytes: Option<usize>,
    portrait_count: Option<u8>,
    voice_count: Option<u8>,
}

fn persona_asset_manager_view(shell: &Shell) -> Result<PersonaAssetManagerView, String> {
    let phase = local_asset_phase(shell)?;
    let installed = phase == PersonaAssetPackPhase::Installed;
    let disclosure = sister_assets::disclosure();
    Ok(PersonaAssetManagerView {
        phase,
        disclosure: PersonaAssetDisclosureView {
            release_id: disclosure.release_id,
            host: disclosure.host,
            path: sister_assets::PACK_PATH,
            bytes: disclosure.bytes,
            boundary: disclosure.boundary_zh_tw,
        },
        asset_file_bytes: installed.then_some(sister_assets::INSTALLED_ASSET_BYTES),
        portrait_count: installed.then_some(4),
        voice_count: installed.then_some(8),
    })
}

fn emit_persona_from_disk(app: &tauri::AppHandle, shell: &Shell) {
    let Ok(path) = config_path() else {
        return;
    };
    let Ok(config) = sister_core::config::Config::load(&path) else {
        return;
    };
    let _ = app.emit("persona-changed", persona_view(&config, shell));
}

fn emit_persona_asset_status(app: &tauri::AppHandle, shell: &Shell) {
    // listener 不採信 event payload，收到後會重叫 status command；即使這一刻讀
    // cache 出錯也要通知另一扇設定頁，讓它顯示那個錯而不是留著舊的 Installed。
    let _ = app.emit(
        "persona-assets-changed",
        persona_asset_manager_view(shell).ok(),
    );
}

struct AssetOperationGuard(Arc<AtomicU8>);

impl Drop for AssetOperationGuard {
    fn drop(&mut self) {
        self.0.store(ASSET_IDLE, Ordering::Release);
    }
}

#[tauri::command(async)]
fn persona_asset_status(shell: tauri::State<'_, Shell>) -> Result<PersonaAssetManagerView, String> {
    persona_asset_manager_view(&shell)
}

/// 這支 command 是唯一會連網的產品入口。簽章故意沒有參數：renderer 無法換
/// persona、URL、path、headers 或 body；每次呼叫也只做一次 fixed GET。
#[tauri::command]
async fn persona_asset_install(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<PersonaAssetManagerView, String> {
    let cache = persona_asset_cache_root()
        .ok_or_else(|| "找不到 Persona 素材 cache 路徑，沒有連線。".to_string())?;
    match sister_assets::cache_state(&cache) {
        Ok(sister_assets::CacheState::Installed) => return persona_asset_manager_view(&shell),
        Ok(sister_assets::CacheState::Available | sister_assets::CacheState::RepairNeeded) => {}
        Err(error) => {
            return Err(format!(
                "問不到 Persona 素材 cache 狀態，沒有開始下載：{error}"
            ));
        }
    }
    shell
        .asset_operation
        .compare_exchange(
            ASSET_IDLE,
            ASSET_INSTALLING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map_err(|running| match running {
            ASSET_REMOVING => "正在刪除本機素材，沒有開始另一個下載。".to_string(),
            ASSET_SETTING_VOICE => "正在更新固定台詞語音設定，沒有開始下載。".to_string(),
            _ => "同一份素材包已經在下載；沒有開始第二個請求。".to_string(),
        })?;
    shell.asset_cancel.store(false, Ordering::Release);
    emit_persona_asset_status(&app, &shell);
    emit_persona_from_disk(&app, &shell);

    let installing = Arc::clone(&shell.asset_operation);
    let cancelled = Arc::clone(&shell.asset_cancel);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _guard = AssetOperationGuard(installing);
        sister_assets::download_and_install(&cache, &cancelled, |_| {})
    });
    let result = task
        .await
        .map_err(|_| "素材安裝工作沒有完成；沒有啟用半包。".to_string())?;

    emit_persona_asset_status(&app, &shell);
    emit_persona_from_disk(&app, &shell);
    result.map_err(|error| error.to_string())?;
    persona_asset_manager_view(&shell)
}

#[tauri::command]
fn persona_asset_cancel(shell: tauri::State<'_, Shell>) -> bool {
    let installing = shell.asset_operation.load(Ordering::Acquire) == ASSET_INSTALLING;
    if installing {
        shell.asset_cancel.store(true, Ordering::Release);
    }
    installing
}

/// 撤回先讓 renderer 停掉飛行中的錄音、resolver fail closed，再碰 exact managed
/// cache。本機系統語音的 opt-in 不屬於這個 cache，所以不跟著被改寫。
#[tauri::command]
async fn persona_asset_remove(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<PersonaAssetManagerView, String> {
    let cache =
        persona_asset_cache_root().ok_or_else(|| "找不到 Persona 素材 cache 路徑。".to_string())?;
    shell
        .asset_operation
        .compare_exchange(
            ASSET_IDLE,
            ASSET_REMOVING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map_err(|running| match running {
            ASSET_INSTALLING => "素材仍在下載；先按停止，等它停下來再刪除。".to_string(),
            ASSET_SETTING_VOICE => "正在更新本機聲音設定；完成後再刪除素材。".to_string(),
            _ => "本機素材已經在刪除。".to_string(),
        })?;

    let _ = app.emit("persona-media-stop", ());
    emit_persona_asset_status(&app, &shell);
    emit_persona_from_disk(&app, &shell);

    let removing = Arc::clone(&shell.asset_operation);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _guard = AssetOperationGuard(removing);
        sister_assets::remove(&cache)
    });
    let result = task
        .await
        .map_err(|_| "素材刪除工作沒有完成；素材仍停用。".to_string())?;

    emit_persona_asset_status(&app, &shell);
    emit_persona_from_disk(&app, &shell);
    result.map_err(|error| error.to_string())?;
    persona_asset_manager_view(&shell)
}

/// 聲音是一次明確選擇。打開不會播放；角色日常對話使用 bundled 固定錄音，
/// 動態答案只有另一顆 trusted 按鈕能交給 localService 系統語音。關掉會先送停聲事件。
#[tauri::command]
fn persona_voice_set(
    enabled: sister_core::config::PersonaVoiceEnabled,
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<PersonaView, String> {
    shell
        .asset_operation
        .compare_exchange(
            ASSET_IDLE,
            ASSET_SETTING_VOICE,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map_err(|running| match running {
            ASSET_INSTALLING => "素材仍在下載；完成或停止後再改本機聲音。".to_string(),
            ASSET_REMOVING => "素材正在刪除；完成後再改本機聲音。".to_string(),
            _ => "另一個本機聲音設定仍在寫入。".to_string(),
        })?;
    let _guard = AssetOperationGuard(Arc::clone(&shell.asset_operation));
    let path = config_path()?;
    let (config, ()) = sister_core::config::Config::update(&path, |config| {
        config.set_persona_voice_from_page(enabled);
        Ok(())
    })
    .map_err(|e| format!("{e:#}"))?;
    if !enabled.get() {
        let _ = app.emit("persona-media-stop", ());
    }
    let view = persona_view(&config, &shell);
    let _ = app.emit("persona-changed", view.clone());
    Ok(view)
}

// ---------- 可選 Azure 雲端朗讀 ----------

#[derive(Clone, Serialize)]
struct AzureTtsView {
    /// 綁住 renderer 看到的狀態與下一次朗讀意圖。舊意圖若排到 cancel／mutation
    /// 後才進 native，不能把新 generation 收編成自己的。
    generation: u64,
    /// `false` 時下面四個 config projection 都是 `null`，不是假裝成預設關閉。
    config_readable: bool,
    enabled: Option<bool>,
    region: Option<sister_core::config::AzureTtsRegion>,
    voice: Option<sister_core::config::AzureTtsVoice>,
    endpoint: Option<String>,
    /// present / missing / unreadable / unsupported；未知和不存在不能共用一個 false。
    credential: &'static str,
    /// `None` = 這台機器連 data dir 都問不到；`false` 含未簽、壞檔與舊版本。
    consented: Option<bool>,
    /// 只有目前條文有效時才投影第四張的原始簽署時間；未簽、舊版、壞檔與
    /// 問不到都不能拿一個 0 冒充。下一次朗讀意圖會把它原樣帶回來做 TOCTOU 比對。
    consent_at: Option<sister_core::model::Millis>,
    /// 只是「下一份新答案或 trusted 手動重播有資格送」；讀這份狀態本身永遠不發 request。
    ready: bool,
    config_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AzureTtsExpected {
    generation: u64,
    enabled: bool,
    region: Option<sister_core::config::AzureTtsRegion>,
    voice: sister_core::config::AzureTtsVoice,
    consent_at: Option<sister_core::model::Millis>,
    credential_present: bool,
}

#[derive(Serialize)]
struct AzureTtsAudioView {
    /// 這次 admission 消耗 baseline 後的新一代；renderer 只有驗過精確 +1 才能
    /// 把它當下一個朗讀意圖的 token。
    generation: u64,
    content_type: &'static str,
    audio_bytes: usize,
    data_url: String,
    /// POST 完成不等於 renderer 已開始（或完成）播放。這份 native lease 讓
    /// master stop 排乾一路跨到 WebView 的 begin／playback end。
    presentation_id: String,
}

fn azure_credential_word(state: azure_credential::CredentialState) -> &'static str {
    match state {
        azure_credential::CredentialState::Present => "present",
        azure_credential::CredentialState::Missing => "missing",
        azure_credential::CredentialState::Unreadable => "unreadable",
        azure_credential::CredentialState::Unsupported => "unsupported",
    }
}

fn azure_tts_view(shell: &Shell) -> AzureTtsView {
    let generation = shell.azure_tts_generation.load(Ordering::Acquire);
    let credential_state = azure_credential::state();
    let credential = azure_credential_word(credential_state);
    let consent = shell.data_dir.as_deref().map(sister_core::consent::load);
    let consented = consent.as_ref().map(|consent| consent.allows_azure_tts());
    let consent_at = consent
        .as_ref()
        .filter(|consent| consent.allows_azure_tts())
        .and_then(|consent| consent.azure_tts);
    let loaded = config_path().and_then(|path| {
        sister_core::config::Config::load(&path).map_err(|error| format!("{error:#}"))
    });
    match loaded {
        Ok(config) => {
            let azure = config.shell.azure_tts;
            let ready = azure.enabled
                && azure.region.is_some()
                && credential_state == azure_credential::CredentialState::Present
                && consented == Some(true);
            AzureTtsView {
                generation,
                config_readable: true,
                enabled: Some(azure.enabled),
                region: azure.region,
                voice: Some(azure.voice),
                endpoint: azure.region.map(|region| region.endpoint()),
                credential,
                consented,
                consent_at,
                ready,
                config_error: None,
            }
        }
        Err(error) => AzureTtsView {
            generation,
            config_readable: false,
            enabled: None,
            region: None,
            voice: None,
            endpoint: None,
            credential,
            consented,
            consent_at,
            ready: false,
            config_error: Some(error),
        },
    }
}

fn emit_azure_tts_changed(app: &tauri::AppHandle, shell: &Shell) {
    // payload 沒有 secret（只有 credential 四態）；兩扇 WebView 收到後仍各自
    // 重讀一次 native truth，不能把 event payload 當 cache。
    let _ = app.emit("azure-tts-changed", azure_tts_view(shell));
}

fn next_azure_tts_generation(current: u64) -> u64 {
    if current >= AZURE_TTS_MAX_GENERATION {
        0
    } else {
        current + 1
    }
}

fn stop_azure_tts_intent(app: &tauri::AppHandle, shell: &Shell) {
    {
        let _transition = azure_tts_transition(shell);
        let _ = shell.azure_tts_generation.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| Some(next_azure_tts_generation(current)),
        );
    }
    let _ = app.emit("azure-tts-stop", ());
}

fn azure_tts_admission(shell: &Shell) -> std::sync::MutexGuard<'_, ()> {
    // 這把鎖只負責把 in-process mutation 和 request admission 排序；沒有可被
    // poison 後信任的資料結構。若先前 command panic，收回 guard 繼續走 fail-closed
    // config／consent／credential 檢查，比永久鎖死刪 key／撤回出口安全。
    shell
        .azure_tts_admission
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn azure_tts_transition(shell: &Shell) -> std::sync::MutexGuard<'_, ()> {
    shell
        .azure_tts_transition
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[tauri::command]
fn azure_tts_read(shell: tauri::State<'_, Shell>) -> AzureTtsView {
    let _admission = azure_tts_admission(&shell);
    azure_tts_view(&shell)
}

/// 這三格立即、獨立落地；不經頁尾那份可能已經開很久的 Settings payload。
#[tauri::command]
fn azure_tts_config_set(
    enabled: sister_core::config::AzureTtsEnabled,
    region: Option<sister_core::config::AzureTtsRegion>,
    voice: sister_core::config::AzureTtsVoice,
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<AzureTtsView, String> {
    let _admission = azure_tts_admission(&shell);
    // 在同一個 admission transaction 裡先讓舊朗讀意圖失效，再寫新設定；因此不會
    // 出現「新 generation + 舊設定」的可重用 snapshot。即使寫檔失敗，停掉舊
    // 播放也是較安全、且 UI 會明講保存失敗的結果。
    stop_azure_tts_intent(&app, &shell);
    let path = config_path()?;
    sister_core::config::Config::update(&path, |config| {
        config.set_azure_tts_from_page(enabled, region, voice);
        Ok(())
    })
    .map_err(|error| format!("{error:#}"))?;
    let status = azure_tts_view(&shell);
    let _ = app.emit("azure-tts-changed", status.clone());
    Ok(status)
}

#[tauri::command]
fn azure_tts_key_set(
    key: String,
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<AzureTtsView, String> {
    let _admission = azure_tts_admission(&shell);
    stop_azure_tts_intent(&app, &shell);
    azure_credential::write(key).map_err(|error| error.to_string())?;
    let status = azure_tts_view(&shell);
    let _ = app.emit("azure-tts-changed", status.clone());
    Ok(status)
}

#[tauri::command]
fn azure_tts_key_delete(
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<AzureTtsView, String> {
    let _admission = azure_tts_admission(&shell);
    stop_azure_tts_intent(&app, &shell);
    azure_credential::delete().map_err(|error| error.to_string())?;
    let status = azure_tts_view(&shell);
    let _ = app.emit("azure-tts-changed", status.clone());
    Ok(status)
}

#[tauri::command]
fn azure_tts_cancel(expected_generation: u64, shell: tauri::State<'_, Shell>) -> bool {
    let _transition = azure_tts_transition(&shell);
    cancel_azure_tts_generation(
        expected_generation,
        &shell.azure_tts_generation,
        &shell.azure_tts_active_generation,
    )
}

fn cancel_azure_tts_generation(
    expected_generation: u64,
    generation: &AtomicU64,
    active_generation: &AtomicU64,
) -> bool {
    let next = next_azure_tts_generation(expected_generation);
    // 還沒 admitted：全域仍停在朗讀意圖看見的 baseline，直接消耗它，讓排隊中的
    // speak 第一行失效。
    if generation
        .compare_exchange(
            expected_generation,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        return true;
    }
    // 已 admitted：speak 已把全域推到 baseline+1，active 仍精確記著 baseline。
    // 只有這兩格同時命中，A cancel 才能再推一次；A 晚到而 B 已開始時 active
    // 會是 B 的 baseline，不能誤殺 B。
    if active_generation.load(Ordering::Acquire) != expected_generation {
        return false;
    }
    let after_cancel = next_azure_tts_generation(next);
    generation
        .compare_exchange(next, after_cancel, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn transport_region(region: sister_core::config::AzureTtsRegion) -> sister_tts::AzureRegion {
    match region {
        sister_core::config::AzureTtsRegion::EastAsia => sister_tts::AzureRegion::EastAsia,
        sister_core::config::AzureTtsRegion::SoutheastAsia => {
            sister_tts::AzureRegion::SoutheastAsia
        }
        sister_core::config::AzureTtsRegion::JapanEast => sister_tts::AzureRegion::JapanEast,
    }
}

fn transport_voice(voice: sister_core::config::AzureTtsVoice) -> sister_tts::Voice {
    match voice {
        sister_core::config::AzureTtsVoice::HsiaoChen => sister_tts::Voice::HsiaoChen,
        sister_core::config::AzureTtsVoice::HsiaoYu => sister_tts::Voice::HsiaoYu,
        sister_core::config::AzureTtsVoice::YunJhe => sister_tts::Voice::YunJhe,
    }
}

/// 唯一能碰 native Azure client 的 desktop helper。第一個參數不是 bool 或一份
/// 可在鎖外重放的 snapshot；只有第四張有效時，shared consent transaction 才能
/// 鑄出這份 guard。Helper by-value 吃掉它並跨完整 transport，第二張 cloud-reading
/// 不能代替，caller 也不能在 POST 前先把跨行程鎖丟掉。
fn synthesize_azure(
    consent_guard: sister_core::consent::AzureTtsAdmissionGuard,
    region: sister_core::config::AzureTtsRegion,
    voice: sister_core::config::AzureTtsVoice,
    key: &str,
    text: &str,
) -> Result<Vec<u8>, String> {
    let _permit = consent_guard.permit();
    let result = sister_tts::AzureClient::new()
        .synthesize(transport_region(region), transport_voice(voice), key, text)
        .map(sister_tts::Audio::into_bytes)
        .map_err(|error| error.to_string());
    drop(consent_guard);
    result
}

struct AzureTtsInFlightGuard {
    in_flight: Arc<AtomicBool>,
    active_generation: Arc<AtomicU64>,
    transition: Arc<Mutex<()>>,
}

impl Drop for AzureTtsInFlightGuard {
    fn drop(&mut self) {
        let _transition = self
            .transition
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 先拿掉 request identity 再開 single-flight；cancel 看見沒有 identity 時
        // 最多讓已完成 request 的下一代失效，不會把它誤認成仍在飛。
        self.active_generation
            .store(AZURE_TTS_NO_ACTIVE_GENERATION, Ordering::Release);
        self.in_flight.store(false, Ordering::Release);
    }
}

/// 把最後一次 generation 檢查與真正 blocking transport 放在 mutation/cancel 共用的
/// 同一把 fence 裡。裸 atomic check 後再呼叫 closure 仍有排程縫；這個 helper 的
/// 線性化規則是：mutation 先拿到就不呼叫 transport，transport 先拿到則 mutation
/// 等它完成。因而 mutation 成功回覆後，舊 request 不會才開始送出。
fn run_generation_pinned_azure_transport<T>(
    transition: &Mutex<()>,
    generation_state: &AtomicU64,
    request_generation: u64,
    transport: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _transport_commit = transition
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation_state.load(Ordering::Acquire) != request_generation {
        return Err("Azure 朗讀已取消；沒有開始 POST。".to_string());
    }
    transport()
}

/// 唯一 outbound command。每次都重讀四道 gate；任何一格缺失都在建立 client 前
/// 返回。blocking transport 無法被假裝成可中止：取消只推進 generation，closure
/// 在 POST 前與回應後各檢查一次，已開始的 request 可能仍跑到 45 秒 global timeout。
#[tauri::command]
async fn azure_tts_speak(
    text: String,
    expected: AzureTtsExpected,
    shell: tauri::State<'_, Shell>,
) -> Result<AzureTtsAudioView, String> {
    let data_dir = shell
        .data_dir
        .as_deref()
        .ok_or_else(|| "找不到資料目錄，問不到第四張同意書；沒有送出 request。".to_string())?;
    // Azure 也是答案正文出境，不可借「本機朗讀」的名字繞過 master stop。
    // 這份 activity guard 活過完整 blocking transport：stop 可先發佈 pending、
    // 立刻拒絕後續工作，但成功回條一定等這份 stop 前已准入的 POST 收尾。
    let master_stop_admission = admit_desktop_brain(Some(data_dir), "這次 Azure 朗讀")?;
    // 整份 expected 來自 renderer 最近一次讀到的 native view，不是在 command 終於
    // 被 poll 時才現場領一張。舊意圖若排在 cancel／mutation 後面才進來，第一行
    // 就拒絕，不能把已經更新的 gate 收編成自己的。
    if shell.azure_tts_generation.load(Ordering::Acquire) != expected.generation {
        return Err("Azure 朗讀狀態已經變更；這次朗讀意圖沒有送出 POST。下一個新答案會使用最新狀態，也可以稍後手動重播。".to_string());
    }
    let _admission = azure_tts_admission(&shell);
    if shell.azure_tts_generation.load(Ordering::Acquire) != expected.generation {
        return Err("Azure 朗讀狀態已經變更；這次朗讀意圖沒有送出 POST。下一個新答案會使用最新狀態，也可以稍後手動重播。".to_string());
    }
    let path = config_path()?;
    let config = sister_core::config::Config::load(&path).map_err(|error| format!("{error:#}"))?;
    let azure = config.shell.azure_tts;
    if expected.enabled != azure.enabled
        || expected.region != azure.region
        || expected.voice != azure.voice
    {
        return Err(
            "Azure 開關、區域或聲音已經變更；這次朗讀意圖沒有送出 POST。下一個新答案會使用最新狀態，也可以稍後手動重播。".to_string(),
        );
    }
    if !azure.enabled {
        return Err("Azure 雲端朗讀目前關閉；沒有送出 request。".to_string());
    }
    let region = azure
        .region
        .ok_or_else(|| "還沒選 Azure Speech 區域；沒有送出 request。".to_string())?;
    // 空白／過大／XML 非法在 consent、credential 與 transport 之前就停。
    sister_tts::build_ssml(transport_voice(azure.voice), &text)
        .map_err(|error| error.to_string())?;
    let consent_guard = match sister_core::consent::begin_azure_tts_admission(data_dir) {
        sister_core::consent::AzureTtsAdmissionConsent::Allowed(guard) => {
            if expected.consent_at != Some(guard.signed_at()) {
                return Err(
                    "第四張 Azure 朗讀同意書已經變更；這次朗讀意圖沒有送出 POST。下一個新答案會使用最新狀態，也可以稍後手動重播。"
                        .to_string(),
                );
            }
            guard
        }
        sister_core::consent::AzureTtsAdmissionConsent::NotAllowed(consent) => {
            let actual_at = consent
                .allows_azure_tts()
                .then_some(consent.azure_tts)
                .flatten();
            if expected.consent_at != actual_at {
                return Err(
                    "第四張 Azure 朗讀同意書已經變更；這次朗讀意圖沒有送出 POST。下一個新答案會使用最新狀態，也可以稍後手動重播。"
                        .to_string(),
                );
            }
            return Err("第四張 Azure 朗讀同意書沒有生效；沒有送出 request。".to_string());
        }
        sister_core::consent::AzureTtsAdmissionConsent::Unknown(error) => {
            return Err(format!(
                "問不到第四張 Azure 朗讀同意書；沒有送出 request：{error:#}"
            ));
        }
    };
    debug_assert!(consent_guard.belongs_to(data_dir));
    let credential = azure_credential::read();
    let credential_present = matches!(&credential, Ok(Some(_)));
    if expected.credential_present != credential_present {
        return Err(
            "Windows Credential Manager 裡的 Azure key 狀態已經變更；這次朗讀意圖沒有送出 POST。下一個新答案會使用最新狀態，也可以稍後手動重播。"
                .to_string(),
        );
    }
    let secret = credential
        .map_err(|error| {
            format!("讀不到 Windows Credential Manager 裡的金鑰；沒有送出 request：{error}")
        })?
        .ok_or_else(|| {
            "Windows Credential Manager 裡沒有 Azure Speech key；沒有送出 request。".to_string()
        })?;

    let request_generation = next_azure_tts_generation(expected.generation);
    let master_stop_boundary;
    {
        let _transition = azure_tts_transition(&shell);
        // `azure_tts_transition` 可能正在等前一份 blocking transport 最長 45 秒；
        // 等它時只持 activity，不持 turnstile。否則第二份 speak 會讓 stop 連 pending
        // 都發佈不了。拿到 Azure fence 後才走最後 master boundary：若 stop 已先
        // pending，這份排隊中的 POST 就在 generation/in-flight mutation 前退出。
        master_stop_boundary = master_stop_admission.boundary().ok_or_else(|| {
            "全停已在 Azure POST 排程前生效；這次朗讀沒有送出 request。".to_string()
        })?;
        shell
            .azure_tts_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                "上一個 Azure POST 還沒結束；沒有開始第二個 request。取消後，已開始的 POST 仍可能跑到逾時，請稍後再按。"
                    .to_string()
            })?;
        shell
            .azure_tts_active_generation
            .store(expected.generation, Ordering::Release);
        if shell
            .azure_tts_generation
            .compare_exchange(
                expected.generation,
                request_generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            shell
                .azure_tts_active_generation
                .store(AZURE_TTS_NO_ACTIVE_GENERATION, Ordering::Release);
            shell.azure_tts_in_flight.store(false, Ordering::Release);
            return Err("Azure 朗讀在入場時已取消；沒有送出 POST。".to_string());
        }
    }
    let active_generation = Arc::clone(&shell.azure_tts_active_generation);
    let in_flight = Arc::clone(&shell.azure_tts_in_flight);
    let in_flight_guard = AzureTtsInFlightGuard {
        in_flight,
        active_generation,
        transition: Arc::clone(&shell.azure_tts_transition),
    };
    let generation_state = Arc::clone(&shell.azure_tts_generation);
    let transport_transition = Arc::clone(&shell.azure_tts_transition);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let _in_flight_guard = in_flight_guard;
        let bytes = run_generation_pinned_azure_transport(
            &transport_transition,
            &generation_state,
            request_generation,
            || {
                let key = secret.as_str().map_err(|error| error.to_string())?;
                // Consent guard 與 transport fence 都跨完整 blocking transport：任何
                // 設定／key／consent mutation 成功回覆後，這份舊 snapshot 不會才 POST。
                synthesize_azure(consent_guard, region, azure.voice, key, &text)
            },
        )?;
        if generation_state.load(Ordering::Acquire) != request_generation {
            return Err("Azure 朗讀已取消；POST 可能已完成，但回應已丟掉、沒有播放。".to_string());
        }
        // Activity guard 跟 bytes 一起交回 async command；不能在 worker 結束時先
        // drop，否則 stop-all 可能先回成功，renderer 才收到 MP3 並第一次播放。
        Ok((bytes, master_stop_admission))
    });
    // worker 已排進 executor；此後它是已准入的舊工作。放 turnstile 讓 stop 發佈
    // pending，activity guard 則留在 worker 到 response／error 全部收乾淨。
    drop(master_stop_boundary);
    // process admission 只排到 worker 已拿到 generation-pinned 工作為止。真正
    // transport commit 另與 mutation/cancel 共用 transition fence：它們若輸給已開始的
    // transport，可能等到最長 45 秒；成功回覆後舊 snapshot 絕不可能才開始 POST。
    // Consent 的跨行程 shared guard 也留到 transport 結束，涵蓋 CLI 撤回。
    drop(_admission);
    let (bytes, master_stop_admission) = task
        .await
        .map_err(|_| "Azure 朗讀工作沒有完成；沒有可播放的回應。".to_string())??;
    let audio_bytes = bytes.len();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(AzureTtsAudioView {
        generation: request_generation,
        content_type: sister_tts::AUDIO_CONTENT_TYPE,
        audio_bytes,
        data_url: format!("data:{};base64,{encoded}", sister_tts::AUDIO_CONTENT_TYPE),
        presentation_id: hold_presentation(master_stop_admission),
    })
}

#[cfg(test)]
mod azure_tts_mapping_tests {
    use super::*;

    #[test]
    fn config_and_transport_share_the_same_exact_regions_and_voices() {
        for region in [
            sister_core::config::AzureTtsRegion::EastAsia,
            sister_core::config::AzureTtsRegion::SoutheastAsia,
            sister_core::config::AzureTtsRegion::JapanEast,
        ] {
            let transport = transport_region(region);
            assert_eq!(region.as_str(), transport.id());
            assert_eq!(region.host(), transport.host());
            assert_eq!(region.endpoint(), transport.endpoint());
        }
        for voice in [
            sister_core::config::AzureTtsVoice::HsiaoChen,
            sister_core::config::AzureTtsVoice::HsiaoYu,
            sister_core::config::AzureTtsVoice::YunJhe,
        ] {
            assert_eq!(voice.as_str(), transport_voice(voice).short_name());
        }
    }

    #[test]
    fn cancel_only_consumes_the_intent_or_the_request_that_came_from_it() {
        let generation = AtomicU64::new(7);
        let active = AtomicU64::new(AZURE_TTS_NO_ACTIVE_GENERATION);
        assert!(cancel_azure_tts_generation(7, &generation, &active));
        assert_eq!(generation.load(Ordering::Acquire), 8);

        // A 已 admitted：全域是 A baseline + 1，而 active 還記著 A baseline。
        active.store(8, Ordering::Release);
        generation.store(9, Ordering::Release);
        assert!(cancel_azure_tts_generation(8, &generation, &active));
        assert_eq!(generation.load(Ordering::Acquire), 10);

        // B 已用下一個 baseline 入場後，晚到的 A cancel 不得推進 B 的 generation。
        active.store(10, Ordering::Release);
        generation.store(11, Ordering::Release);
        assert!(!cancel_azure_tts_generation(8, &generation, &active));
        assert_eq!(generation.load(Ordering::Acquire), 11);
    }

    #[test]
    fn renderer_safe_generation_wrap_keeps_cancel_pairing_exact() {
        let generation = AtomicU64::new(0);
        let active = AtomicU64::new(AZURE_TTS_MAX_GENERATION);
        assert!(cancel_azure_tts_generation(
            AZURE_TTS_MAX_GENERATION,
            &generation,
            &active,
        ));
        assert_eq!(generation.load(Ordering::Acquire), 1);
    }

    #[test]
    fn transport_commit_fence_has_no_check_then_post_window() {
        use std::sync::mpsc;

        // Mutation already linearized: the transport closure is never reached.
        let transition = Mutex::new(());
        let generation = AtomicU64::new(8);
        {
            let _mutation = transition.lock().expect("mutation fence");
            generation.store(9, Ordering::Release);
        }
        let sent = AtomicBool::new(false);
        let rejected = run_generation_pinned_azure_transport(&transition, &generation, 8, || {
            sent.store(true, Ordering::Release);
            Ok(())
        });
        assert!(rejected.is_err());
        assert!(!sent.load(Ordering::Acquire));

        // Transport already linearized: the same fence stays held for the entire fake
        // transport, so a mutation/cancel cannot report success in the middle and leave
        // a not-yet-started POST behind it.
        let transition = Arc::new(Mutex::new(()));
        let generation = Arc::new(AtomicU64::new(12));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker_transition = Arc::clone(&transition);
        let worker_generation = Arc::clone(&generation);
        let worker = std::thread::spawn(move || {
            run_generation_pinned_azure_transport(
                &worker_transition,
                &worker_generation,
                12,
                || {
                    entered_tx.send(()).expect("announce fake transport");
                    release_rx.recv().expect("release fake transport");
                    Ok(())
                },
            )
        });
        entered_rx.recv().expect("fake transport entered");
        assert!(
            transition.try_lock().is_err(),
            "transition fence must live across the transport closure"
        );
        release_tx.send(()).expect("finish fake transport");
        assert_eq!(worker.join().expect("worker did not panic"), Ok(()));
        assert!(transition.try_lock().is_ok());
    }
}

/// 設定頁上看得到、改得動的那幾項。
///
/// **刻意只是設定檔的一個子集。** 截圖間隔、去重門檻那些沒有放進來，因為它們
/// 改了要重開 `record` 才生效（見 `Recorder::set_privacy`）——一個按了儲存卻
/// 要等重開才生效、而且沒說的欄位，比沒有那個欄位更糟。
///
/// `[brain]` 由上面的 CLI 登入卡獨立、原子地寫；頁尾「儲存」完全不碰它。
/// 這樣登入測通後，不會被一張較早載入的設定表用舊值蓋回去。
#[derive(Serialize, Deserialize)]
struct Settings {
    excluded_apps: Vec<String>,
    excluded_urls: Vec<String>,
    excluded_titles: Vec<String>,
    pause_on_screenshare: bool,
    redact_clipboard_secrets: bool,
    query_log: bool,
    frames_days: u32,
    text_days: u32,
    persona_enabled: sister_core::config::PersonaVisible,
    persona_id: sister_core::config::PersonaId,
    persona_motion: sister_core::config::PersonaMotionEnabled,
    persona_tap_lines: sister_core::config::PersonaTapLinesEnabled,
    /// 設定檔實際的位置。給人看的——她說她存到哪，就要指得出來是哪一個檔案。
    ///
    /// **只出不進**：存檔時路徑一律由 `config_path()` 重算，不是相信視窗傳回來
    /// 的那個字串。一個「要寫到哪個檔案」由前端決定的介面，等於讓那一頁指到
    /// 任何一個地方去。`default` 讓存檔的 payload 不必回傳它。
    #[serde(default)]
    path: String,
}

fn config_path() -> Result<PathBuf, String> {
    sister_core::config::Config::default_path().ok_or_else(|| "找不到設定檔路徑".to_string())
}

#[tauri::command]
fn settings_read() -> Result<Settings, String> {
    let path = config_path()?;
    let c = sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))?;
    Ok(Settings {
        excluded_apps: c.privacy.excluded_apps,
        excluded_urls: c.privacy.excluded_urls,
        excluded_titles: c.privacy.excluded_titles,
        pause_on_screenshare: c.privacy.pause_on_screenshare,
        redact_clipboard_secrets: c.privacy.redact_clipboard_secrets,
        query_log: c.privacy.query_log,
        frames_days: c.retention.frames_days,
        text_days: c.retention.text_days,
        persona_enabled: c.shell.persona.visible(),
        persona_id: c.shell.persona.id,
        persona_motion: c.shell.persona.motion_enabled(),
        persona_tap_lines: c.shell.persona.tap_lines_enabled(),
        path: path.display().to_string(),
    })
}

#[tauri::command(async)]
async fn brain_cli_read(shell: tauri::State<'_, Shell>) -> Result<brain_cli::BrainCliView, String> {
    let path = config_path()?;
    let state = Arc::clone(&shell.brain_cli_state);
    tauri::async_runtime::spawn_blocking(move || brain_cli::read_view(&path, &state))
        .await
        .map_err(|error| format!("讀取 CLI 狀態：{error}"))?
}

#[tauri::command(async)]
async fn brain_cli_connect(
    provider: String,
    shell: tauri::State<'_, Shell>,
) -> Result<brain_cli::BrainCliOutcome, String> {
    let provider = sister_core::provider_cli::BrainProvider::from_id(&provider)
        .ok_or_else(|| format!("不認得的 CLI：{provider}"))?;
    let path = config_path()?;
    let sister = recorder_supervisor::recorder_path()?;
    let state = Arc::clone(&shell.brain_cli_state);
    // 在 blocking worker 排入佇列之前就占住槽；取消不會落在 worker 尚未
    // 啟動的空窗，下一筆登入也不能同時穿過去。
    let claim = brain_cli::begin(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        brain_cli::connect(provider, &path, &sister, claim)
    })
    .await
    .map_err(|error| format!("CLI 登入工作中止：{error}"))?
}

#[tauri::command(async)]
async fn brain_cli_test(
    shell: tauri::State<'_, Shell>,
) -> Result<brain_cli::BrainCliOutcome, String> {
    let path = config_path()?;
    let state = Arc::clone(&shell.brain_cli_state);
    let claim = brain_cli::begin(&state)?;
    tauri::async_runtime::spawn_blocking(move || brain_cli::test_selected(&path, claim))
        .await
        .map_err(|error| format!("CLI 測試工作中止：{error}"))?
}

#[tauri::command]
fn brain_cli_cancel(shell: tauri::State<'_, Shell>) -> bool {
    brain_cli::cancel(&shell.brain_cli_state)
}

/// 她問「我一個人在跑的時候，可不可以自己按網址」那一格（PHASES #42）。
///
/// **這一格不是設定頁裡的一個開關，是她開口問的一個問題。** 差別在預設值：
/// 開關有一邊是預設的，而預設的那一邊等於產品替他選了。這裡沒有預設值——
/// `answered` 是 `None` 就是**還沒問過**，而那和「他說了不要」是兩句不同的話。
///
/// 每一句話都從 `sister_hands::url_policy` 拿，一個字都不在這裡寫。同一題
/// `sister url-policy` 也在問，兩份文案分家的話他答的是哪一個沒人說得準。
#[derive(Serialize)]
struct UrlPolicyView {
    /// 她問的那一句。
    question: &'static str,
    /// 他現在的答案的 key；`None` = **還沒問過**，不是一個答案。
    answered: Option<&'static str>,
    /// 兩個答案。順序就是 `UrlOpenAnswer::ALL`，不在前端重排。
    options: Vec<UrlPolicyOption>,
    /// 他回答之前她怎麼做。**這一句不能省**：少了它，一個從來沒被問過的人
    /// 會以為現在這個行為是他自己選的。
    before_you_answer: &'static str,
    /// 答案會被存到哪裡。他要看得到那個檔案。
    path: String,
}

#[derive(Serialize)]
struct UrlPolicyOption {
    key: &'static str,
    line: &'static str,
}

fn url_policy_view(
    path: &Path,
    answered: Option<sister_hands::url_policy::UrlOpenAnswer>,
) -> UrlPolicyView {
    UrlPolicyView {
        question: sister_hands::url_policy::QUESTION,
        answered: answered.map(|a| a.key()),
        options: sister_hands::url_policy::UrlOpenAnswer::ALL
            .into_iter()
            .map(|a| UrlPolicyOption {
                key: a.key(),
                line: a.line(),
            })
            .collect(),
        before_you_answer: sister_hands::url_policy::BEFORE_YOU_ANSWER,
        path: path.display().to_string(),
    }
}

#[tauri::command]
fn url_policy_read() -> Result<UrlPolicyView, String> {
    let path = config_path()?;
    url_policy_read_at(&path)
}

fn url_policy_read_at(path: &Path) -> Result<UrlPolicyView, String> {
    // 設定檔還不存在是正常狀態（他剛裝好）。**但那不是一個答案**，所以這裡
    // 走預設的 `Config`，而它那一欄仍然是 `None`。
    let config = match path
        .try_exists()
        .map_err(|e| format!("檢查設定 {}：{e}", path.display()))?
    {
        true => sister_core::config::Config::load(path).map_err(|e| format!("{e:#}"))?,
        false => sister_core::config::Config::default(),
    };
    Ok(url_policy_view(path, config.hands.url_open))
}

/// 他按了其中一顆。回傳的是**她複述他選的那一句**。
///
/// `key` 認不得就整個拒絕，不挑一個最像的：挑了等於替他做決定，而且做完
/// 之後畫面會顯示他「答過了」。前端只送 `UrlOpenAnswer::ALL` 裡的 key，
/// 但這一格是 IPC 邊界，前端說什麼都不算數。
#[tauri::command]
fn url_policy_write(key: String) -> Result<String, String> {
    let path = config_path()?;
    url_policy_write_at(&path, &key)
}

fn url_policy_write_at(path: &Path, key: &str) -> Result<String, String> {
    let answer = sister_hands::url_policy::UrlOpenAnswer::from_key(key)
        .ok_or_else(|| format!("這不是我問的那兩個答案之一，沒有存：{key}"))?;
    // **先讀再改再寫**，和 `settings_write` 同一條紀律：從空白組一份會把這一
    // 格沒畫出來的欄位（排除規則、保留天數……）全部重設成預設值。
    sister_core::config::Config::update(path, |config| {
        config.hands.url_open = Some(answer);
        Ok(())
    })
    .map_err(|e| format!("{e:#}"))?;
    Ok(answer.recorded_line())
}

#[cfg(test)]
mod url_policy_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_config(label: &str) -> (PathBuf, PathBuf) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sister-desktop-url-policy-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp config dir");
        let path = dir.join("config.toml");
        (dir, path)
    }

    #[test]
    fn missing_answer_is_null_and_both_shared_answers_are_visible() {
        let (dir, path) = temp_config("read");
        let view = url_policy_read_at(&path).expect("first run is readable");
        let json = serde_json::to_value(&view).expect("serialize view");
        assert!(json["answered"].is_null(), "{json}");
        assert_eq!(json["options"].as_array().map(Vec::len), Some(2));
        assert_eq!(json["question"], sister_hands::url_policy::QUESTION);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn writing_an_answer_preserves_every_setting_this_question_does_not_show() {
        let (dir, path) = temp_config("write");
        let mut config = sister_core::config::Config::default();
        config.privacy.excluded_apps = vec!["keepass.exe".into(), "bank.exe".into()];
        config.retention.text_days = 777;
        config.save(&path).expect("seed config");

        let before_invalid = std::fs::read(&path).unwrap();
        assert!(url_policy_write_at(&path, "yes-please").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before_invalid);

        for answer in sister_hands::url_policy::UrlOpenAnswer::ALL {
            let line = url_policy_write_at(&path, answer.key()).expect("write answer");
            assert!(line.contains(answer.line()), "{line}");
            let back = sister_core::config::Config::load(&path).expect("read back");
            assert_eq!(back.hands.url_open, Some(answer));
            assert_eq!(back.privacy.excluded_apps, ["keepass.exe", "bank.exe"]);
            assert_eq!(back.retention.text_days, 777);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_config_is_an_error_not_an_unanswered_question() {
        let (dir, path) = temp_config("malformed");
        std::fs::write(&path, "not = [valid").unwrap();
        assert!(url_policy_read_at(&path).is_err());
        assert!(url_policy_write_at(&path, "only-on-my-press").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// 把 CLI 產生的完整 eval report 縮成開發者指標頁能看的那一層。
///
/// 瀏覽器端會讀使用者明確選的檔案，再把內容送進這支 command。這裡
/// 不收路徑，所以一扇開發頁不會變成可以任意讀本機檔案的介面。
/// 回傳型別也刻意不含 report 裡任何自由字串；解析失敗也不把原值抄進畫面。
#[tauri::command(async)]
fn eval_report_view(contents: String) -> Result<sister_core::eval::MetricsView, String> {
    sister_core::eval::metrics_view_from_json(&contents)
        .map_err(|_| "JSON 格式或 eval report 版本不符合這一版 sister".to_string())
}

/// 存完之後，「她什麼時候會照這份跑」的答案。
///
/// 存好之後那句話以前寫死是「正在跑的 record 會在 5 秒內換上這一份」。那句
/// 話有一個沒被檢查的前提：**真的有一個 record 在跑**。一個剛裝好、還沒按過
/// 「開始記錄」的人，看到的是一句承諾一件不會發生的事的話——他改完排除規則，
/// 以為門從此關著，然後才去按開始（那時候倒是真的生效了），或者根本不去按。
///
/// 而更難看的是反過來：他**以為**她在錄（工作管理員裡有那個行程、系統匣有
/// 圖示），實際上那個行程幾分鐘前就掛了。這句話會替那件事背書。
#[derive(Serialize)]
struct WriteOutcome {
    /// 心跳現在說什麼：`"recording"`／`"booting"`／`"thinking"`／`"none"`／
    /// `"unreadable"`。決定那句話怎麼講。
    ///
    /// **五個值，不是一個布林。** 上一版是 `recording: bool`（`is_recording`），
    /// 於是開機那幾分鐘這一頁說「現在沒有人在錄，所以這一份要等你按下**開始
    /// 記錄**才會生效」——而那顆按鈕在那幾分鐘按下去只會回一句「已經有一個
    /// sister record 在跑了」（見 [`start_recording`] 那道 `is_occupied` 閘
    /// 門）。一句在他剛改完排除規則的那一刻、指著一條走不通的路的話。
    watching: &'static str,
    /// 設定已落盤但 Persona event 沒能送出，和「存檔失敗」是兩件事。
    /// `true` 只代表 Tauri 接受這次 emit，不冒充另一扇 WebView 已經套用。
    persona_event_emitted: bool,
}

#[tauri::command]
fn settings_write(
    settings: Settings,
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<WriteOutcome, String> {
    let path = config_path()?;
    // **先讀再改再寫**，不是從空白組一份出來。設定檔裡有這一頁沒有畫出來的
    // 欄位（截圖間隔、每日畫面額度……），從頭組一份會把它們全部重設成預設值
    // ——使用者只是改了個保留天數，磁碟預算卻被悄悄換掉了。
    let (c, ()) = sister_core::config::Config::update(&path, |c| {
        c.privacy.excluded_apps = settings.excluded_apps;
        c.privacy.excluded_urls = settings.excluded_urls;
        c.privacy.excluded_titles = settings.excluded_titles;
        c.privacy.pause_on_screenshare = settings.pause_on_screenshare;
        c.privacy.redact_clipboard_secrets = settings.redact_clipboard_secrets;
        c.privacy.query_log = settings.query_log;
        c.retention.frames_days = settings.frames_days;
        c.retention.text_days = settings.text_days;
        c.set_persona_from_page(
            settings.persona_enabled,
            settings.persona_id,
            settings.persona_motion,
            settings.persona_tap_lines,
        );
        Ok(())
    })
    .map_err(|e| format!("{e:#}"))?;
    // 存成功才換角色。設定頁和字母人是兩扇 WebView；少了這個事件，畫面會直到
    // 整支 desktop 重開才跟 config.toml 一致。payload 仍只含表達層資料，沒有
    // 排除規則、OCR、答案或 action。
    let persona_event_emitted = app
        .emit("persona-changed", persona_view(&c, &shell))
        .is_ok();
    // 存成功之後才問。反過來的話，一個存不進去的檔案會拿到一句「5 秒內換上」。
    Ok(WriteOutcome {
        watching: shell
            .data_dir
            .as_ref()
            .map(|dir| {
                sister_core::heartbeat::watching_word(sister_core::heartbeat::presence(
                    dir,
                    sister_core::now_ms(),
                ))
            })
            .unwrap_or("none"),
        persona_event_emitted,
    })
}

/// 這台機器上，這幾條規則到底生不生效。
///
/// [`lint_url_rules`] 檢查的是**一條規則自己**寫得對不對。這一支檢查的是另一
/// 件事：整組規則會不會因為機器讀不到網址而**一條都不生效**。兩者都通過的
/// 使用者，看到的是一份綠色的清單和一個以為關上了的門。
///
/// 探測是 recorder 做的（只有它有 UIA），所以這裡讀的是它留下來的報告，而且
/// 拿現在這一刻的設定重算——見 [`sister_core::capabilities`]。
#[derive(Serialize)]
struct PrivacyHealth {
    /// 現在這份設定裡，有哪幾件事其實沒在做。空的 = 都生效。
    ///
    /// 每一則都帶著 `about`，因為它們該掛在這一頁上**不同的區塊**底下——
    /// 見 [`sister_core::capabilities::About`]。
    broken: Vec<sister_core::capabilities::Broken>,
    /// 報告是什麼時候探的。`None` = **還沒有報告**（沒錄過、或那個檔案被刪
    /// 了）——那和「都生效」是兩件事，畫面上不准長得一樣。
    at: Option<i64>,
    /// `capture.enabled = false`：她連開始都不會開始。
    ///
    /// 這一格問的是「這幾條規則生不生效」，而總開關關著的時候那個問題沒有
    /// 意義——**一條都不會被用到，因為根本沒有畫面進來**。而它在畫面上和
    /// 「一切正常」長得一模一樣：規則清單是綠的、這一格是空的。使用者以為
    /// 他設好了一台會記錄、而且會避開網銀的機器；他有的是一台什麼都不記的。
    ///
    /// 這個欄位不能塞進 `broken`：那個清單講的是「你以為關上的門其實開著」，
    /// 而這件事的方向剛好相反（整棟房子是空的）。混在一起會讓那幾句話變成
    /// 一堆語氣一樣、輕重不分的字。
    capture_off: bool,
    /// 輸入 hook 是已知可用、已知不可用，還是這份報告沒量到。
    ///
    /// 另存原始三態，不從 `broken` 是否有一句話反推：清單空白可能是
    /// `Available`，也可能是 `Unknown`。
    input_hook: sister_core::capabilities::CapabilityState,
    /// 那幾條規則**驗過了沒有**。沒有報告時是 `Unknown`，
    /// `at` 同時說明連一份報告都沒有。
    ///
    /// 同樣不能塞進 `broken`，同樣是因為方向不同：那個清單講「門開著」，這裡
    /// 講「我還不知道門關了沒」。而**「不知道」在這一頁上一直長得像「沒問
    /// 題」**——`broken` 是空的、這一格就是空白，而這一頁自己寫著「空白在這
    /// 一格就是『都生效』」。見 [`sister_core::capabilities::UrlRules`]。
    url_rules: sister_core::capabilities::UrlRules,
}

#[tauri::command]
fn privacy_health(
    urls: Vec<String>,
    shell: tauri::State<'_, Shell>,
) -> Result<PrivacyHealth, String> {
    let path = config_path()?;
    let mut config = sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))?;
    // 拿**輸入框裡現在這一刻**的規則去問，不是設定檔裡存好的那一份——和
    // `lint_url_rules` 同一條紀律。他正在打第一條的時候就該知道它不會生效；
    // 等他按了儲存才講晚了一步，而那一步裡他已經相信門關上了。
    config.privacy.excluded_urls = urls;
    let capture_off = !config.capture.enabled;
    let report = shell
        .data_dir
        .as_ref()
        .and_then(|dir| sister_core::capabilities::read(dir));
    Ok(match report {
        Some(r) => PrivacyHealth {
            broken: r.broken_privacy_rules(&config.privacy),
            at: Some(r.at),
            capture_off,
            input_hook: r.input_hook,
            url_rules: r.url_rules_verdict(&config.privacy),
        },
        None => PrivacyHealth {
            broken: Vec::new(),
            at: None,
            capture_off,
            input_hook: sister_core::capabilities::CapabilityState::Unknown,
            url_rules: sister_core::capabilities::UrlRules::Unknown,
        },
    })
}

/// 哪幾條網址規則寫了也不會命中。
///
/// **這是這一頁最有價值的一格。** 排除規則最糟的失效方式不是漏寫，是寫了一條
/// 自以為有效的——使用者看著清單上那一行，以為網銀已經擋掉了。同一份判斷
/// `sister doctor` 和 `record` 都在用，這裡只是把它搬到打字的當下。
#[tauri::command]
fn lint_url_rules(rules: Vec<String>) -> Vec<(String, String)> {
    sister_core::config::suspicious_url_rules(&rules)
}

/// 那張畫面本身。
///
/// **這是這個產品的重點，不是附加功能。** 她說「你三天前看過這個」的時候，
/// 使用者要能當場翻回去看——不然那句話跟任何一個會唬爛的東西沒有差別。
///
/// 圖用 data URL 送過去而不是開一個檔案協定：少一個要設 scope 的表面，
/// 而且「哪些檔案讀得到」的答案就變成「只有這一行指到的那一張」。
#[tauri::command(async)]
fn frame_image(frame_id: i64, shell: tauri::State<'_, Shell>) -> Result<FrameView, String> {
    // `frames.image_path` 存的是**相對**路徑（`2026/08/19/0-….png`），因為整個
    // `frames/` 目錄要能整包搬走、整包備份。所以讀它之前一定要接上根目錄。
    //
    // 少了這一段的時候，`fs::read` 拿行程的工作目錄當根——那是他按下捷徑的
    // 那個資料夾，永遠不會是資料目錄。於是**每一次**點出處都得到「圖不見了」，
    // 而那張圖好端端地躺在磁碟上。這一支的文件第一行寫著「這是這個產品的重點，
    // 不是附加功能」，而它從來沒有成功過一次。
    let root = shell
        .data_dir
        .as_deref()
        .map(sister_core::config::Config::frames_dir)
        .ok_or_else(|| "找不到資料目錄，讀不到那張畫面".to_string())?;
    with_db(&shell, |db| {
        let ctx = db
            .frame_context(frame_id)
            .map_err(|e| format!("{e:#}"))?
            .ok_or_else(|| "找不到這張畫面".to_string())?;

        // 「有這一筆但沒有圖」是**正常**的，不是錯誤。差別要講清楚，不然
        // 使用者會以為程式壞了。
        //
        // 不說是哪一種原因：這裡看到的只有一個 NULL，而 NULL 底下躺著四件
        // 事（只記字、截圖節流、每日額度、保留期到了）。挑一個講出來有四分
        // 之三的機會是錯的。
        let rel = ctx
            .image_path
            .ok_or_else(|| "這一筆沒有留下畫面，只有文字".to_string())?;
        let path = root.join(&rel);
        // 路徑要印出來，而且是**接好根目錄之後**的那一條。使用者拿它去檔案總管
        // 貼上就知道到底有沒有那個檔——這是他唯一能自己驗證這句話的辦法。
        let bytes =
            std::fs::read(&path).map_err(|e| format!("圖不見了：{}（{e}）", path.display()))?;

        let ext = std::path::Path::new(&rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("webp");
        Ok(FrameView {
            data_url: format!("data:image/{ext};base64,{}", sister_shell::base64(&bytes)),
            ts: ctx.ts,
            app: ctx.app_id,
            title: ctx.window_title,
            url: ctx.url,
        })
    })
}

// ---------- 全域暫停熱鍵 ----------

/// 熱鍵現在到底有沒有搶到手。
///
/// **這個結構存在的唯一理由是「搶不到」這件事會發生。** 全域熱鍵是先搶先贏的：
/// 同一組 `Ctrl+Alt+P` 可能早就被螢幕錄影軟體、輸入法或另一個常駐程式拿走了，
/// 而作業系統不會告訴使用者是誰拿走的——它只會讓他按下去、什麼都沒發生。
///
/// 對一顆**暫停**鍵來說那是最壞的一種壞法：他以為她停了，她還在錄。所以註冊
/// 的結果要一路送到設定頁上，寫成一句話，而不是塞進一行只有 `--verbose` 才
/// 看得到的 log。
#[derive(Clone, Serialize, Default)]
struct HotkeyView {
    /// 設定檔裡寫的那一組。空字串 = 使用者關掉了它。
    wanted: String,
    /// 現在真的按得動嗎。
    registered: bool,
    /// 沒搶到的話，作業系統或 Tauri 給的原因。
    reason: Option<String>,
    /// 剛剛試了、但沒搶到的那一組。[`wanted`](Self::wanted) 已經退回上一組。
    ///
    /// 存在的理由是 [`apply_hotkey`] 會先 `unregister_all()`。搶不到的時候
    /// 如果就這樣結束，**連本來好好的那一組也一起沒了**——而畫面上還顯示著
    /// 一組既沒註冊也沒寫進設定檔的組合，下次開機又默默變回舊的那個。
    /// 使用者看到的只有「剛剛試的那組沒成功」，完全不知道暫停鍵從此失效。
    rejected: Option<String>,
    /// 開機時讀不出設定檔，所以 [`wanted`](Self::wanted) 是**內建預設值**，
    /// 不是他設的那一組。帶著讀失敗的原因。
    ///
    /// 開機那段以前是 `Config::load(&p).ok().unwrap_or_default()`——一個
    /// 手寫壞掉的 `config.toml`（或被 OneDrive 鎖住、磁碟滿）會讓他設的
    /// `Ctrl+Alt+S` 安靜地變成內建的那一組。症狀是：他按 S 沒有反應，而
    /// 設定頁指著另一組說「搶到了」，兩邊都不提設定檔。
    ///
    /// 對一顆**暫停**鍵來說這是最壞的一種壞法，和 [`hotkey_set`] 那段註解
    /// 講的是同一件事：他以為她停了，她還在錄。
    config_unreadable: Option<String>,
    hands_wanted: String,
    hands_registered: bool,
    hands_reason: Option<String>,
    hands_collided: bool,
}

struct Hotkey(Mutex<HotkeyView>);

/// 把設定檔裡那一組換上去，回報結果。
///
/// 先 `unregister_all` 再註冊：這一支同時被開機和「設定頁上改了一組」呼叫，
/// 不先拆掉舊的話，改過三次之後會有三組熱鍵同時活著——其中兩組是他以為自己
/// 已經取消掉的。
fn apply_hotkey(app: &tauri::AppHandle, wanted: &str, hands_wanted: &str) -> HotkeyView {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let configured_wanted = wanted.trim().to_string();
    let plan = sister_hands::kill_switch::plan_hotkeys(wanted, hands_wanted);
    let wanted_to_register = plan.pause.clone().unwrap_or_default();
    let hands_wanted = plan.hands.clone().unwrap_or_default();
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();

    // 空字串是一個正當的選擇，不是壞掉的設定：全域熱鍵會從所有程式手上把那個
    // 組合搶走，所以要留一條關掉它的路。這裡不回報 reason，因為沒有失敗。
    // 安全鍵先註冊。若相等判定仍有漏網，第二次的 AlreadyRegistered 會落在可由
    // 系統匣代替的暫停鍵，而不是拔手鍵。
    let hands_reason = if hands_wanted.is_empty() {
        None
    } else {
        shortcuts
            .on_shortcut(hands_wanted.as_str(), |app, _shortcut, event| {
                if event.state() != tauri_plugin_global_shortcut::ShortcutState::Pressed {
                    return;
                }
                let shell = app.state::<Shell>();
                let outcome = sister_hands::kill_switch::press_hands_hotkey(
                    shell.data_dir.as_deref(),
                    sister_core::now_ms(),
                );
                let says = sister_hands::kill_switch::hands_hotkey_message(&outcome);
                tracing::info!("拔手熱鍵：{says}（{outcome:?}）");
                announce_hands_pulled(app, &says);
                refresh_tray(app);
            })
            .err()
            .map(|e| e.to_string())
    };

    let reason = if wanted_to_register.is_empty() {
        None
    } else {
        shortcuts
            .on_shortcut(wanted_to_register.as_str(), |app, _shortcut, event| {
                // 只認**按下**。少了這一行，按一次會進來兩次（按下 + 放開），
                // 於是暫停立刻被自己取消掉——一顆看起來完全沒反應的熱鍵。
                if event.state() != tauri_plugin_global_shortcut::ShortcutState::Pressed {
                    return;
                }
                match toggle_pause(app.clone(), app.state::<Shell>()) {
                    Ok(paused) => announce_hotkey(app, paused),
                    Err(e) => tracing::error!("熱鍵暫停失敗：{e}"),
                }
            })
            .err()
            .map(|e| e.to_string())
    };

    let registered = !wanted_to_register.is_empty() && reason.is_none();
    let hands_registered = !hands_wanted.is_empty() && hands_reason.is_none();
    HotkeyView {
        // 撞號時政策層不註冊暫停，但這一格仍要如實顯示設定檔裡的組合。
        wanted: configured_wanted,
        registered,
        reason,
        rejected: None,
        config_unreadable: None,
        hands_registered,
        hands_wanted,
        hands_reason,
        hands_collided: plan.collided.is_some(),
    }
}

/// 熱鍵按下去之後，讓他看得到結果。
///
/// 系統匣的選單字會變、視窗裡的字母人會變灰，但**兩個他都可能看不到**——熱鍵
/// 存在的理由正是「她不在畫面上的時候我也想按」。所以停下來的那一下要把她叫
/// 出來：一個看得到的灰色字母人，就是那句「好，我停了」。
///
/// 只在**停**的方向叫，不在恢復的方向叫。停下來按錯了是隱私問題，恢復按錯了
/// 只是白按一次；而每次切換都彈一個視窗出來，會讓收進系統匣這個動作失去意義。
fn announce_hotkey(app: &tauri::AppHandle, paused: bool) {
    if !paused {
        return;
    }
    if let Some(win) = app.get_webview_window(PET) {
        // 不 `set_focus`：他按下暫停的那一刻，螢幕上多半正有一件他在做的事，
        // 把游標從那件事上搶走不是幫忙。
        let _ = win.show();
    }
}

/// 拔手熱鍵按下去之後，讓他**讀到**那句話。
///
/// [`announce_hotkey`] 那一半靠「字母人變灰」就講完了——暫停只有兩種狀態。
/// 拔手不是：`press_hands_hotkey` 有四種結局，其中兩種（沒寫成而她其實已經
/// 停了／沒寫成而手真的還接著）的下一步完全相反，而**灰不灰分不出它們**。
/// 所以這一半一定要把整句話送過去。
///
/// 走的是既有的那條 `notice`（前端 `noticeAboutHer`），不是新發明的機制：
/// 那一格本來就是「他手指剛剛按下去的那一下」的位置。事件另取名字而不是借
/// `recorder-failed`，是因為這一句多半不是失敗，借那個名字會讓事件名自己說謊。
///
/// `show()` 不 `set_focus()`，和上面同一個理由。
fn announce_hands_pulled(app: &tauri::AppHandle, says: &str) {
    let _ = app.emit("hands-pulled", says.to_string());
    if let Some(win) = app.get_webview_window(PET) {
        let _ = win.show();
    }
}

#[tauri::command]
fn hotkey_state(hotkey: tauri::State<'_, Hotkey>) -> HotkeyView {
    hotkey.0.lock().expect("hotkey").clone()
}

/// 換一組熱鍵：先真的去搶，搶到了才寫進設定檔。
///
/// 順序是刻意的。反過來寫（先存再註冊）的話，一組搶不到的熱鍵會留在設定檔裡，
/// 下次開機再失敗一次——而使用者早就把那一頁關掉了。
///
/// **搶不到的時候要把舊的那組裝回去。** [`apply_hotkey`] 開頭就
/// `unregister_all()` 了，所以「試了一組被別人佔走的組合」以前的後果是連本來
/// 好好的那一組也一起消失：`Ctrl+Alt+P` 本來按得動，他試了一次 `Ctrl+Alt+S`
/// ——從那一刻起暫停熱鍵完全失效，設定頁顯示一組設定檔裡不存在的組合，下次
/// 開機又默默變回 `Ctrl+Alt+P`。而畫面上那句話讓他以為只有剛剛試的那組沒成功。
///
/// 對一顆暫停鍵來說那是最壞的一種壞法：他以為她停了，她還在錄。
#[tauri::command]
fn hotkey_set(
    app: tauri::AppHandle,
    combo: String,
    hotkey: tauri::State<'_, Hotkey>,
) -> Result<HotkeyView, String> {
    let loaded = config_path()
        .and_then(|path| sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}")));
    let current = hotkey.0.lock().expect("hotkey").clone();
    let previous = current.wanted.clone();
    let config_error = loaded.as_ref().err().cloned();
    let hands_wanted = loaded
        .map(|config| config.shell.hands_stop_shortcut)
        .unwrap_or_else(|_| current.hands_wanted.clone());
    let view = apply_hotkey(&app, &combo, &hands_wanted);
    if let Some(error) = config_error {
        let restored = HotkeyView {
            // **沿用開機那一次的旗標，不要在這裡自己生一個。**
            //
            // 這個欄位的意思是「現在用的是**內建預設值**，不是他設的那一組」，
            // 設定頁（`settings.js` 的 `paintHandsHotkey`）就是照這個意思寫句子的。
            // 只有開機那條路會讓它成立：那裡是 `loaded.unwrap_or_default().shell`，
            // 真的退到了出廠值。
            //
            // 這條路正好相反：上面那句 `unwrap_or_else(|_| current.hands_wanted…)`
            // 保住的就是**他設的那一組**。在這裡填 `Some(error)` 的話，拔手那一格
            // 會紅著寫「暫停和拔手現在用的都是內建預設值，不是你設的」，而它正上方
            // 的格子印著他自己那顆、那顆還真的按得動。他會改去按出廠的
            // `Ctrl+Alt+H`——那顆沒註冊，按下去什麼都不會發生。而且那句話會**黏著**
            // 到下一次成功換熱鍵為止，中間每一次 `reloadHotkey()` 都再畫一次。
            //
            // 沿用之後兩條路都是真的：開機失敗過就一直說（值確實是預設值），開機
            // 沒失敗過就不說——這一輪讀失敗的事由底下那句 `Err` 交代，而它連拔手鍵
            // 現在是哪一組都講得出來。
            config_unreadable: current.config_unreadable.clone(),
            ..apply_hotkey(&app, &previous, &hands_wanted)
        };
        let still = if restored.registered {
            format!(
                "現在還在用 {}。",
                sister_shell::pretty_combo(&restored.wanted)
            )
        } else if restored.hands_collided {
            // 撞號要自己一臂。`wanted` 現在裝的是設定檔裡那一組（撞號時也有值），
            // 少了這一臂會掉到最後那個 else，把「讓給拔手了」講成「被別的程式
            // 搶走了」——歸因是假的，而他會去找那個不存在的程式。
            format!(
                "現在原來那組 {} 和拔手鍵撞號，讓給拔手了；改用系統匣裡的暫停。",
                sister_shell::pretty_combo(&restored.wanted)
            )
        } else if restored.wanted.is_empty() {
            "現在暫停熱鍵是關掉的。".to_string()
        } else {
            format!(
                "現在原來那組 {} 也搶不到；改用系統匣裡的暫停。",
                sister_shell::pretty_combo(&restored.wanted)
            )
        };
        let hands_still = if restored.hands_registered {
            format!(
                "拔手鍵現在還在用 {}。",
                sister_shell::pretty_combo(&restored.hands_wanted)
            )
        } else if restored.hands_wanted.is_empty() {
            "拔手鍵現在是關掉的。".to_string()
        } else {
            format!(
                "拔手鍵 {} 現在也搶不到；請從系統匣拔手。",
                sister_shell::pretty_combo(&restored.hands_wanted)
            )
        };
        *hotkey.0.lock().expect("hotkey") = restored;
        return Err(format!(
            "設定檔讀不出來；剛剛試的 {} 沒有存下來。暫停鍵{still}{hands_still}\n{error}",
            sister_shell::pretty_combo(&combo)
        ));
    }
    let action = sister_hands::kill_switch::hotkey_set_action(
        view.hands_collided,
        view.registered,
        view.wanted.is_empty(),
    );
    // `hotkey_set_action` 的八格在 sister-hands 裡有執行覆蓋；這個 match 把純決策
    // 接回 Tauri 的註冊、寫檔與 state，仍沒有執行覆蓋。改純函式的任一格會紅；
    // 把這裡的 `RestoreCollision` 接到別臂，crate 測試不會知道；
    // `check-settings-say.mjs` ⑳ 會用原始碼形狀針抓到，但那不是行為覆蓋。
    let view = match action {
        sister_hands::kill_switch::HotkeySetAction::Persist => {
            let persist = || -> Result<(), String> {
                let path = config_path()?;
                sister_core::config::Config::update(&path, |c| {
                    c.shell.pause_shortcut = view.wanted.clone();
                    Ok(())
                })
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
            };
            // 存不進去的時候**不可以直接 `?` 出去**。那三行以前是裸的 `?`，於是
            // 新的那組已經真的搶下來了（`apply_hotkey` 開頭就 `unregister_all()`），
            // 而底下那行「把結果寫回 state」永遠跑不到——`hotkey_state` 從此回報
            // 舊的那組 `registered: true`，設定頁照著印「搶到了。現在按 Ctrl+Alt+P
            // 都會暫停或繼續」。真正會暫停的是他剛剛試的那一組，P 是死的。下次
            // 開機又從設定檔讀回 P，所以這個分歧不留下任何痕跡。
            //
            // 而且這是那顆**暫停**鍵。上面那段註解說這一格最壞的壞法是「他以為
            // 她停了，她還在錄」——這條路正好走到那裡。
            //
            // 什麼時候會走到這裡：設定檔壞掉（手寫的 retention = 0）、防毒或
            // OneDrive 鎖著 config.toml、磁碟滿。
            if let Err(e) = persist() {
                let restored = apply_hotkey(&app, &previous, &hands_wanted);
                // `pretty_combo` 而不是原樣印：這一整串是塞進 `Err(String)` 直接
                // 上畫面的，設定頁不會替它排版。原樣印出來是「還在用 Ctrl+Alt+KeyP」
                // ——而鍵盤上沒有一顆鍵叫 KeyP。他要照著這句話去按的。
                let still = if restored.registered {
                    format!("還在用 {}。", sister_shell::pretty_combo(&restored.wanted))
                } else if restored.hands_collided {
                    // 走得到：設定檔裡兩顆本來就同一組（開機就撞號），他在設定頁換成
                    // 一組不撞的、搶到了、但存不進去 → 這裡把**原來那組**裝回去，而
                    // 原來那組正是撞號的那一組。少了這一臂會掉進最後那個 else，
                    // 把「讓給拔手了」講成「被別的程式搶走了」。
                    format!(
                        "而舊的那組 {} 和拔手鍵撞號，讓給拔手了——改用系統匣裡的暫停。",
                        sister_shell::pretty_combo(&restored.wanted)
                    )
                } else if restored.wanted.is_empty() {
                    "熱鍵本來就是關掉的，維持原狀。".to_string()
                } else {
                    "而舊的那組現在也搶不到了——改用系統匣裡的暫停。".to_string()
                };
                *hotkey.0.lock().expect("hotkey") = restored;
                return Err(format!(
                    "搶到了，但存不進設定檔，所以退回原來那一組。{still}\n{e}"
                ));
            }
            view
        }
        sister_hands::kill_switch::HotkeySetAction::RestoreCollision => {
            let mut restored = apply_hotkey(&app, &previous, &hands_wanted);
            restored.rejected = Some(combo);
            restored.hands_collided = true;
            restored
        }
        sister_hands::kill_switch::HotkeySetAction::RestoreRejected => {
            // 設定檔沒動過，所以退回去的一定是設定檔裡那一組。`rejected` 帶著他
            // 剛剛打的那個組合，讓那句話講得出「你試的那組沒搶到，還在用舊的」。
            HotkeyView {
                rejected: Some(view.wanted),
                ..apply_hotkey(&app, &previous, &hands_wanted)
            }
        }
    };
    *hotkey.0.lock().expect("hotkey") = view.clone();
    Ok(view)
}

/// 從同步系統匣 event handler 開一扇 WebView。
///
/// Tauri / WebView2 在 Windows 有一條明列的 deadlock：在 event handler
/// 裡直接 `WebviewWindowBuilder::build` 會卡在 controller callback，只留下
/// 有標題、全白、`Not Responding` 的原生窗。系統匣 callback 只准走到
/// 這裡；真正建視窗在獨立 OS thread 裡做。
fn spawn_window(
    app: tauri::AppHandle,
    description: &'static str,
    open: fn(tauri::AppHandle) -> Result<(), String>,
) {
    let _window_thread = std::thread::spawn(move || {
        if let Err(e) = open(app) {
            tracing::error!("{description}開不起來：{e}");
        }
    });
}

/// 真正建立設定頁的 internal helper。
///
/// Windows WebView2 在 synchronous command 或 event handler 裡直接建
/// `WebviewWindow` 會 deadlock。這支所以不是 command：IPC 入口由底下
/// 的 async wrapper 叫，系統匣則一律經 [`spawn_window`]。
fn open_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&SETTINGS_WINDOW_OPENING) else {
        return Ok(());
    };
    const SETTINGS: &str = "settings";
    if let Some(win) = app.get_webview_window(SETTINGS) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        SETTINGS,
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title("AI-Sister 設定")
    .inner_size(640.0, 720.0)
    .min_inner_size(460.0, 420.0)
    .build()
    .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

/// 開設定頁。同一個 label 重複用，所以按兩次不會得到兩個視窗。
#[tauri::command]
async fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    open_settings_window(app)
}

// ---------- 四張同意書 ----------

#[derive(Serialize)]
struct SheetView {
    key: String,
    /// 條文本身。**從 core 拿，不在這裡重打一份**——同一句話在 CLI 和視窗上
    /// 長得不一樣的話，「他到底同意了哪一句」就沒有答案了。
    wording: String,
    without: String,
    granted_at: Option<i64>,
    /// 現在算不算數。簽過但條文改版了的話，`granted_at` 有值而這裡是 false。
    effective: bool,
    /// 目前條文是否已被明確回答；不同意時 `effective` 仍是 false，但不會在
    /// 每次啟動重新追問。授權與「問過了」不能共用同一個 bool。
    reviewed: bool,
}

#[derive(Serialize)]
struct ConsentView {
    path: String,
    current: bool,
    allows_recording: bool,
    /// 第三張**同意書**的狀態：可不可以留圖。
    allows_frames: bool,
    /// 設定檔的 `capture.store_images`。`None` = 那個檔案讀不出來。
    ///
    /// 同意是**上限**，不是開關：這一張簽了、設定檔卻關著的時候，硬碟上一張
    /// 截圖都不會多。只看 `allows_frames` 的那一頁會說「而且會留截圖」，而那
    /// 是一句他要去翻 frames/ 才戳得破的假話。
    ///
    /// 讀不出來就是 `None`，不是猜一個預設值：預設是 true，猜錯的方向正好是
    /// 「答應了一件不會發生的事」。
    store_images: Option<bool>,
    /// 設定檔的 `capture.enabled`——那個總開關。`None` = 檔案讀不出來。
    ///
    /// 它關著的時候每個 tick 直接回 `Tick::Disabled`，連螢幕都不會碰。而這一
    /// 頁簽完名的最後一句話是「接下來跑 sister record 她才會開始，而且會留
    /// 截圖」——**兩個子句都錯**，而他要等上一整天才會發現。
    ///
    /// 和 `store_images` 分開送，因為那兩件事要講的話不一樣：一個是「她根本
    /// 不會開始」，一個是「她會開始，但只記字」。
    capture_enabled: Option<bool>,
    /// 剛剛那一下**順手把另外三張的簽署時間清掉了**（條文改版）。
    ///
    /// `consent_read` 永遠是 false——只有真的動手的那一下才會是 true。CLI 對
    /// 這件事會印一行 ⚠，這一頁以前完全安靜：他勾了一張，另外三張的「2026 年
    /// 7 月 2 日同意過」就這樣從畫面上消失，沒有人告訴他為什麼。
    reset_by_version: bool,
    sheets: Vec<SheetView>,
}

fn consent_dir<'r>(shell: &tauri::State<'r, Shell>) -> Result<&'r std::path::Path, String> {
    // `inner()` 而不是直接 deref：借的是**受管理的那份 state**（活得和 app 一樣
    // 久），不是這個 `State` 包裝的區域變數。
    shell
        .inner()
        .data_dir
        .as_deref()
        // 問不出資料目錄的時候**不要**猜一個。同意書寫錯地方，等於他按了同意
        // 而 `sister record` 永遠讀不到——一顆按了沒用、卻顯示成功的按鈕。
        .ok_or_else(|| "找不到資料目錄，同意書沒有地方可以存".to_string())
}

fn consent_view(dir: &std::path::Path) -> ConsentView {
    consent_view_after(dir, false)
}

fn consent_view_after(dir: &std::path::Path, reset_by_version: bool) -> ConsentView {
    use sister_core::consent::Sheet;
    let c = sister_core::consent::load(dir);
    // 只讀一次設定檔。分兩次讀的話，兩個欄位有機會來自不同的兩份內容
    // ——他正好在這中間存檔的話，畫面上會出現一個檔案裡沒有的組合。
    let config = config_path()
        .ok()
        .and_then(|p| sister_core::config::Config::load(&p).ok());
    ConsentView {
        reset_by_version,
        path: sister_core::consent::path(dir).display().to_string(),
        current: c.current(),
        allows_recording: c.allows_recording(),
        allows_frames: c.allows_frames(),
        store_images: config.as_ref().map(|c| c.capture.store_images),
        capture_enabled: config.as_ref().map(|c| c.capture.enabled),
        sheets: Sheet::ALL
            .into_iter()
            .map(|s| SheetView {
                key: s.key().to_string(),
                wording: s.wording().to_string(),
                without: s.without().to_string(),
                granted_at: c.get(s),
                effective: c.effective(s),
                reviewed: c.reviewed(s),
            })
            .collect(),
    }
}

#[tauri::command]
fn consent_read(shell: tauri::State<'_, Shell>) -> Result<ConsentView, String> {
    Ok(consent_view(consent_dir(&shell)?))
}

/// 勾或不勾其中一張。
///
/// 一次只動一張，而且每一下都馬上落地——「按了四個勾再按確定」的做法，會在
/// 他關掉視窗的那一刻讓前幾個勾消失，而他以為都存好了。
#[tauri::command]
fn consent_set(
    key: String,
    granted: bool,
    app: tauri::AppHandle,
    shell: tauri::State<'_, Shell>,
) -> Result<ConsentView, String> {
    use std::str::FromStr;
    let dir = consent_dir(&shell)?;
    let sheet = sister_core::consent::Sheet::from_str(&key)?;
    let changing_azure = sheet == sister_core::consent::Sheet::AzureTts;
    let _azure_admission = changing_azure.then(|| azure_tts_admission(&shell));
    if changing_azure {
        // 拿到 admission transaction 後，先讓已排隊但還沒 admitted 的舊朗讀意圖失效，
        // 再在同一個 transaction 裡寫同意書。
        // Grant 也必須做：未簽時建立的舊意圖不能延遲到 grant 後，拿同一代 token
        // 借到剛新增的權限才 POST。
        // 寫檔失敗時仍停止舊播放；比讓使用者按了撤回卻在錯誤回條後又聽到聲音安全。
        stop_azure_tts_intent(&app, &shell);
    }
    if !granted && sheet == sister_core::consent::Sheet::LocalRecording {
        // 先送一個零 I/O 的 typed message，讓 worker 在 consent/stop 寫檔可能失敗
        // 之前就永久取消 pending Login/watchdog。若 worker 已停，不存在 automatic
        // spawn；若它正忙到十秒沒回，message 仍在唯一 channel 裡，底下 durable
        // revoke barrier 照常前進，不能因 supervisor 回條慢而拒絕使用者撤回。
        if let Ok(recorder) = recorder_handle(shell.inner())
            && let Err(error) = recorder.cancel_automatic_for_consent_revoke()
        {
            tracing::warn!("撤回前無法即時取得 recorder supervisor 回條：{error}");
        }
    }
    let mut reset_by_version = false;
    let mut revoking_recording = false;
    let mut revoke_barrier_written = false;
    let mut revoke_barrier_clear = None;
    let committed = sister_core::consent::mutate(dir, |c| {
        // 這份 c 是拿到跨行程 write lock 後才重讀的；設定頁和 CLI 同時動不同
        // 張時，後來的 writer 只能接著最新版本改，不能把鎖外舊快照裡的第一張
        // 簽名蓋回來。
        let allowed_before = c.allows_recording();
        // 條文改版之後，舊的那幾張不能跟著新的一起被存成「現在這一版簽的」。
        // 和 CLI 那邊同一個決定：整份清掉，只留他這次真的按下去的。
        //
        // **而且要講出來。** CLI 對這件事印一行 ⚠，這一頁以前完全安靜——他勾了
        // 一張，另外三張的「2026 年 7 月 2 日同意過」就從畫面上消失了。
        reset_by_version = !c.current() && *c != sister_core::consent::Consent::default();
        if !c.current() {
            *c = sister_core::consent::Consent::default();
        }
        if granted {
            c.grant(sheet, sister_core::now_ms());
        } else {
            c.revoke(sheet);
        }
        revoking_recording = allowed_before && !c.allows_recording();
        if revoking_recording {
            // 獨立 barrier 在 consent save **之前**先落地；即使後面的 atomic
            // replace 失敗、舊 consent 仍是 Allowed，所有 start/recorder 也會先
            // 看見這個 barrier。它不覆寫人工 Stop/DesktopQuit marker。
            sister_core::control::request_consent_revoke(dir).map_err(|error| {
                anyhow::anyhow!(
                    "第一張同意書要撤回，但 durable revoke barrier 寫入沒有完整成功：{error:#}"
                )
            })?;
            revoke_barrier_written = true;
        }
        if granted && sheet == sister_core::consent::Sheet::LocalRecording && c.allows_recording() {
            // 票必須在 consent writer lock 裡、用這次即將 commit 的 snapshot 取得；
            // 真正清理只能等 mutate 成功後。generation 會擋住較舊 regrant 清掉
            // 隨後到達的新 revoke。
            revoke_barrier_clear = sister_core::control::prepare_consent_revoke_barrier_clear(dir)?;
        }
        Ok(())
    });
    committed.map_err(|error| {
        if revoke_barrier_written {
            format!(
                "撤回 barrier 已留下，但同意書 transaction 沒有完整成功；record/start 仍會被 barrier 擋住：{error:#}"
            )
        } else if revoking_recording {
            format!(
                "第一張同意書沒有改動，而且撤回 barrier 寫入沒有完整成功；目前無法證明停止條件已可靠落地：{error:#}"
            )
        } else {
            format!("{error:#}")
        }
    })?;
    if let Some(ticket) = revoke_barrier_clear {
        match ticket.clear_after_commit() {
            Ok(
                sister_core::control::ConsentRevokeBarrierClear::Cleared
                | sister_core::control::ConsentRevokeBarrierClear::AlreadyAbsent,
            ) => {}
            Ok(sister_core::control::ConsentRevokeBarrierClear::Superseded) => {
                return Err(
                    "第一張同意書已存好，但另一個較新的撤回已取代這張清理票；revoke barrier 仍在，沒有恢復自動記錄。"
                        .to_owned(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "第一張同意書已存好，但 revoke barrier 清不掉；沒有恢復自動記錄：{error:#}"
                ));
            }
        }
    }
    // 任一張改動都可能是 version reset；另一扇設定頁收到後自己重讀第四張真相。
    emit_azure_tts_changed(&app, &shell);
    Ok(consent_view_after(dir, reset_by_version))
}

/// 真正建立同意書頁的 internal helper。只准從 async command、
/// [`spawn_window`] 或 Tauri 明確允許同步建視窗的 setup hook 進來。
fn open_onboarding_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&ONBOARDING_WINDOW_OPENING) else {
        return Ok(());
    };
    const ONBOARDING: &str = "onboarding";
    if let Some(win) = app.get_webview_window(ONBOARDING) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        ONBOARDING,
        tauri::WebviewUrl::App("onboarding.html".into()),
    )
    .title("四張同意書")
    .inner_size(620.0, 720.0)
    .min_inner_size(460.0, 480.0)
    .build()
    .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

/// 開同意書那一頁。同一個 label 重複用。
#[tauri::command]
async fn open_onboarding(app: tauri::AppHandle) -> Result<(), String> {
    open_onboarding_window(app)
}

/// 一次刪除的規模。給人看的，所以欄位名是中文語意上的那幾個東西。
#[derive(Serialize)]
struct Erasure {
    chunks: u64,
    facts: u64,
    frames: u64,
    images: u64,
    image_bytes: u64,
    events: u64,
    /// 那段時間裡他自己問過的話（題庫）。單獨一項，理由見
    /// `PruneReport::queries_deleted`。
    queries: u64,
    /// 那段時間結束之後**一列都不剩**的那幾場錄製。
    ///
    /// 刪的不是內容，是「那天 13:02 到 17:44 她在錄」——一份沒有任何內容、
    /// 卻證明他那段時間坐在電腦前的紀錄。那張表以前誰都不刪，而這一頁那顆
    /// 按鈕上寫的是「忘掉」。見 `retention::delete_empty_sessions`。
    sessions: u64,
    /// 那段時間裡她按你的指示動過幾次手（`action-log.jsonl`）。
    ///
    /// **這一欄不在資料庫裡**，所以 `From<PruneReport>` 給不出它，兩個呼叫端
    /// 各自要補。上面那一段講題庫、畫面紀錄、錄製紀錄的註解，講的都是同一個
    /// 故事：一類東西被刪掉了、卻沒有出現在這張清單上。這是第四次，而這一次
    /// 那類東西是完整的網址和檔案路徑。
    actions: u64,
    /// 讀不懂、問不出時間，因此忘掉時也會被刪掉的 action-log 列數。
    actions_unreadable: u64,
    /// 存著的授權書不屬於時間區間；按下忘掉仍會整張刪除。
    grant: bool,
    /// 刪不掉的檔案。**不吞掉**：那幾張截圖還躺在磁碟上，而使用者以為
    /// 它們已經不在了。
    failed: Vec<String>,
    /// 資料庫說有圖、磁碟上找不到那個檔。
    ///
    /// 不是失敗（東西確實不在了），但**也不是刪掉了**。少了這一欄，預覽說
    /// 「12 張畫面（1.8 MB）」而結果一張都沒提，中間那個落差沒有人解釋——
    /// 而那正是他拿來對帳的兩個數字。CLI 早就有這一行。
    missing: u64,
    /// 刪完之後**沒被帶走**的那幾列 `sessions`——見
    /// [`DbStats::only_session_shells_left`](sister_core::db::DbStats::only_session_shells_left)。
    ///
    /// `None` 不是 0：預覽算不出這個（它一列都不動，沒有「刪完之後」可言）。
    /// 兩種意思寫成同一個 0 的話，這裡就變成它自己要修的那個 bug——`Some(0)`
    /// 是「沒有東西留下來」，`None` 是「這一趟沒有問這個問題」。
    sessions_left: Option<u64>,
    /// 留下來的那一列是誰的：`"live"`（她此刻正在錄）、`"booting"`（有一個
    /// recorder 正在起來，那一列不是它的）、`"unreadable"`（心跳讀不懂）、
    /// `"gone"`（沒有人在，她當掉了）。
    ///
    /// 只有 `sessions_left > 0` 的時候有意義，所以它和上面那一欄要在同一個
    /// `if` 裡讀完——分開讀就會有人拿一個沒問過的值去講一句斷言。
    ///
    /// # 為什麼是三個字串，不是一個布林
    ///
    /// 上一版是 `shell_is_live: bool`，算的是 `heartbeat::is_occupied`，而註解
    /// 把那個選擇寫成有理由的（「正在開機的 recorder 也佔著這個目錄」）。於是
    /// 畫面印的是「此刻有人佔著這個資料目錄（**她正在錄，或正在開機**）」——那
    /// 個「或」正是這個 repo 一路在刪的東西，而且它在開機那幾分鐘是**假的**：
    /// `BootBeat::start` 先寫心跳，`start_session` 最後才 INSERT，所以那幾分鐘
    /// 裡手上這一列一定是**上一次當機留下來的殼**，不是佔著目錄的那一個。三種
    /// 心跳三句話，而一個布林湊不出三種答案。CLI 那邊是同一個判斷
    /// （`session_shell_why` 收 `Option<Phase>`）。
    ///
    /// 一個欄位三個值，不是兩個布林：兩個布林拼得出「又在錄又在開機」這種不存
    /// 在的組合，而拼錯的那一次不會有人紅。
    shell_beat: &'static str,
}

impl From<sister_core::retention::PruneReport> for Erasure {
    fn from(r: sister_core::retention::PruneReport) -> Self {
        Self {
            chunks: r.chunks_deleted,
            facts: r.facts_deleted,
            frames: r.frames_deleted,
            images: r.images_deleted,
            image_bytes: r.image_bytes_freed,
            events: r.events_deleted,
            queries: r.queries_deleted,
            sessions: r.sessions_deleted,
            failed: r.failed,
            missing: r.missing,
            // action log 不在資料庫裡，`PruneReport` 看不到它。兩個呼叫端各自
            // 問一次 `ActionLog`，所以這裡只能是 0——和下面 `sessions_left`
            // 同一個模式：這一支答不出來的，不要在這裡編一個。
            actions: 0,
            actions_unreadable: 0,
            grant: false,
            // 這一支只看得到「刪掉了什麼」。留下什麼要再問一次資料庫，所以
            // 預設是「沒問」，由 `forget_range` 補上。
            sessions_left: None,
            shell_beat: "gone",
        }
    }
}

/// 忘掉這一段會刪掉什麼。一句 DELETE 都沒有。
///
/// 畫面檔的根目錄拿不到就整支拒絕，理由和 `forget_range` **正好相反但一樣硬**：
/// 那邊是不能假裝刪掉了，這邊是不能假裝放得出空間。退成 `None` 的話這一支會
/// 回報「0 個畫面檔」，而真的按下去會刪掉幾百張——一份把代價說小的預覽，比
/// 沒有預覽更糟。
#[tauri::command(async)]
fn forget_preview(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Erasure, String> {
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，算不出這一段會刪掉多少東西".to_string())?;
    let frames = sister_core::config::Config::frames_dir(dir);
    // 預覽也要把她那段時間動過的手算進去；可讀列和讀不懂但仍會被刪掉的列
    // 分開回，和真正刪除的 `ForgetReport` 是同一組數字。
    let actions = sister_hands::ActionLog::in_data_dir(dir)
        .count_in_range(from_ts, to_ts)
        .map_err(|e| format!("{e:#}"))?;
    let grant = sister_hands::semi_action::grant_path(dir).exists()
        || sister_hands::semi_action::grant_tmp_path(dir).exists();
    with_db(&shell, |db| {
        db.forget_preview(from_ts, to_ts, Some(&frames))
            .map(|report| Erasure {
                actions: actions.removed_in_range,
                actions_unreadable: actions.removed_unreadable,
                grant,
                ..Erasure::from(report)
            })
            .map_err(|e| format!("{e:#}"))
    })
}

/// 真的刪。**沒有回收桶，沒有復原。**
///
/// 前端會先叫一次 `forget_preview` 把數字擺在使用者眼前，但那個順序是前端的
/// 禮貌，不是這裡的前提——這一支不管有沒有人預覽過都會照做，因為「一定要先
/// 預覽」的規則放在畫面上就等於沒有規則。真正的防線在 core：區間反過來的話
/// 一列都不動。
#[tauri::command(async)]
fn forget_range(
    from_ts: i64,
    to_ts: i64,
    shell: tauri::State<'_, Shell>,
) -> Result<Erasure, String> {
    // 畫面檔的根目錄拿不到就整支拒絕，**不要**退成 `None` 硬幹。
    // `None` 的意思是「只刪資料庫、不碰檔案」，那會回報一份漂亮的成功，
    // 而那段時間的截圖一張不少地留在磁碟上。
    let dir = shell
        .data_dir
        .as_ref()
        .ok_or_else(|| "找不到資料目錄，不能保證截圖真的會被刪掉".to_string())?;
    let frames = sister_core::config::Config::frames_dir(dir);
    // 在借出資料庫之前問，因為它讀的是磁碟上的心跳檔，不是資料庫。**問
    // 要保留 Thinking；它和「當掉」的下一步不同。
    let beat = match sister_core::heartbeat::watching_word(sister_core::heartbeat::presence(
        dir,
        sister_core::now_ms(),
    )) {
        "recording" => "live",
        other => other,
    };
    // 資料庫和畫面之外，`action-log.jsonl` 裡也有那一段的完整網址與檔案路徑。
    // 少了這一刀，那句「已經忘掉了」只對一半的磁碟成立。CLI 那邊是同一句話
    // （`crates/sister-cli/src/ops.rs` 的 `forget`），兩邊都要做。
    //
    // 在借資料庫之前先做：這一刀失敗要整個停下來，不能發生「資料庫刪了、
    // 檔案沒刪」而畫面照樣報成功。
    let forgotten = sister_hands::ActionLog::in_data_dir(dir)
        .forget_range(from_ts, to_ts)
        .map_err(|err| format!("{err:#}"))?;
    // 授權書。**同一支函式，CLI 的 `sister forget` 也走它**——兩邊各寫一份
    // 的話，改天多一個檔案只會補到其中一邊，而兩邊都照樣說「已經忘掉了」。
    let grant = !sister_hands::semi_action::forget_saved_grant(dir)
        .map_err(|err| format!("刪除授權書失敗：{err}"))?
        .is_empty();
    with_db_mut(&shell, |db| {
        let report = db
            .forget(from_ts, to_ts, Some(&frames))
            .map_err(|err| format!("{err:#}"))?;
        // **沒被帶走的那一列也要講。** 上一場當掉的話，那一列 `sessions` 撐得
        // 過這一刀（守衛不准碰還沒收尾的最新一列，因為那可能是此刻正在錄的那
        // 一場）。少了這一欄，這一頁只列得出刪掉的東西，他要到別的地方才會撞
        // 見一個「1 場錄製」站在一整排 0 旁邊。CLI 那邊是同一句話。
        let stats = db.stats().map_err(|err| format!("{err:#}"))?;
        let left = if stats.only_session_shells_left() {
            stats.sessions as u64
        } else {
            0
        };
        Ok(Erasure {
            actions: forgotten.removed_in_range,
            actions_unreadable: forgotten.removed_unreadable,
            grant,
            sessions_left: Some(left),
            // 分得出來就不要印「或」。CLI 那邊同一個判斷（`session_shell_why`）。
            // 沒有留下來的列就沒有這個問題——那時候這一欄不准講一個沒問過的
            // 答案，所以跟著 `From` 的預設走。
            shell_beat: if left > 0 { beat } else { "gone" },
            ..Erasure::from(report)
        })
    })
}

/// 開時間軸。
///
/// 為什麼要一個視窗而不是塞進字母人：搜尋回答的是「我記得的那件事在哪」，
/// 時間軸回答的是**「她到底記了什麼」**——後者是使用者決定要不要信任她的
/// 依據，而 340 像素寬的欄位撐不起「翻過一整天」這件事。
///
/// 同一個 label 重複用，所以按兩次不會得到兩個視窗。
fn open_timeline_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&TIMELINE_WINDOW_OPENING) else {
        return Ok(());
    };
    const TIMELINE: &str = "timeline";
    if let Some(win) = app.get_webview_window(TIMELINE) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(
        &app,
        TIMELINE,
        tauri::WebviewUrl::App("timeline.html".into()),
    )
    .title("她記得的每一天")
    .inner_size(980.0, 720.0)
    .min_inner_size(560.0, 420.0)
    .build()
    .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[tauri::command]
async fn open_timeline(app: tauri::AppHandle) -> Result<(), String> {
    open_timeline_window(app)
}

/// 開發者模式才會在系統匣出現的評測摘要頁。
///
/// 它不自己跑評測，也不去猜資料目錄；只讀使用者在頁面上明確選中的
/// `replay evaluate --to` 報告。同一個 label 重複使用，避免開出兩份互相
/// 不知道對方載了哪個檔案的數字。
fn open_metrics_window(app: tauri::AppHandle) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&METRICS_WINDOW_OPENING) else {
        return Ok(());
    };
    const METRICS: &str = "metrics";
    if let Some(win) = app.get_webview_window(METRICS) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(&app, METRICS, tauri::WebviewUrl::App("metrics.html".into()))
        .title("AI-Sister 開發者指標")
        .inner_size(1080.0, 720.0)
        .min_inner_size(720.0, 480.0)
        .build()
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

/// 他點開了第 `rank` 筆的出處。
///
/// 這是檢索品質唯一不需要人工標註就拿得到的訊號：他點下去的那一刻，等於幫那
/// 一題標了正解，而 `rank` 直接說出排序把它放在第幾個。Phase 2 的題庫要「≥ 30
/// 題來自真實 query log」，靠的就是這一筆。
///
/// 和 [`open_frame`] 分開兩個命令，因為它們的失敗方向不一樣：畫面開不起來要
/// 讓他知道，記不進題庫不該打斷他正在做的事。畫面那一邊是 fire-and-forget。
#[tauri::command(async)]
fn log_click(
    query_id: i64,
    chunk_id: i64,
    rank: usize,
    shell: tauri::State<'_, Shell>,
) -> Result<(), String> {
    with_db(&shell, |db| {
        db.log_click(query_id, chunk_id, rank).map_err(|e| {
            tracing::warn!("這一次點擊沒記進題庫：{e}");
            e.to_string()
        })
    })
}

/// 他說「這一題我本來已經忘了」（或者收回那句話）。
///
/// PHASES.md Phase 1 的第一條退場條件是「自用 7 天內 ≥ 3 次答對我自己都忘掉的
/// 東西」，而那件事只有他知道——題庫裡沒有任何一欄答得出來。
///
/// **和 [`log_click`] 的失敗處理相反。** 點擊是 fire-and-forget：他要的是那張
/// 畫面，記不記得到帳是次要的。這裡他要的**就是**記這一筆——記不進去而畫面裝
/// 作記進去了，等於在退場條件的證據上說謊。所以錯誤要回到畫面上。
#[tauri::command(async)]
fn mark_query(query_id: i64, marked: bool, shell: tauri::State<'_, Shell>) -> Result<bool, String> {
    with_db(&shell, |db| {
        db.mark_query(query_id, marked)
            // 這裡只要 `marked`——那顆按鈕問的是「現在該畫成什麼樣子」。
            // `changed`（這一次有沒有真的動到）是終端機才需要分辨的事：那邊
            // 打得出一個他自己想錯的題號，這邊的 id 是剛剛那一次回答帶下來的。
            .map(|o| o.marked)
            .map_err(|e| {
                tracing::warn!("這一次標記沒記進題庫：{e}");
                format!("{e:#}")
            })
    })
}

/// 開一個看圖的視窗。
///
/// 為什麼是另一個視窗：字母人只有 340 像素寬，一張 2560×1440 的畫面縮進去
/// 是一片糊——「點開看當時畫面」看不清楚就等於沒做。
///
/// 同一個 label 重複用，所以連點五筆結果不會得到五個視窗；已經開著就換一張
/// 圖並拉到前面。
fn open_frame_window(app: tauri::AppHandle, frame_id: i64) -> Result<(), String> {
    let Some(_opening) = WindowOpening::claim(&FRAME_WINDOW_OPENING) else {
        return Ok(());
    };
    const VIEWER: &str = "frame";
    let target = format!("frame.html?id={frame_id}");

    if let Some(win) = app.get_webview_window(VIEWER) {
        let _ = win.eval(format!("window.location.replace('{target}')"));
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(&app, VIEWER, tauri::WebviewUrl::App(target.into()))
        .title("當時的畫面")
        .inner_size(1100.0, 720.0)
        .min_inner_size(480.0, 320.0)
        .build()
        .map_err(|e| format!("{e:#}"))?;
    Ok(())
}

#[tauri::command]
async fn open_frame(app: tauri::AppHandle, frame_id: i64) -> Result<(), String> {
    open_frame_window(app, frame_id)
}

#[derive(Serialize)]
struct FrameView {
    data_url: String,
    ts: i64,
    app: Option<String>,
    title: Option<String>,
    url: Option<String>,
}

/// 游標現在在視窗座標的哪裡，CSS 像素。
///
/// `cursor_position` 給的是整個桌面的實體像素，`inner_position` 是視窗左上角
/// 的實體像素，兩者相減再除以縮放比才會和 renderer 的 `getBoundingClientRect()`
/// 同一套。問不出來就回 `None`——那是「不知道」，不是「在原點」。
fn cursor_in_window(win: &tauri::WebviewWindow) -> Option<(i32, i32)> {
    let cursor = win.cursor_position().ok()?;
    let origin = win.inner_position().ok()?;
    let scale = win.scale_factor().ok()?;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let x = (cursor.x - f64::from(origin.x)) / scale;
    let y = (cursor.y - f64::from(origin.y)) / scale;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    // f64 -> i32 在 Rust 是飽和轉換，不會 UB 也不會 panic。
    Some((x.floor() as i32, y.floor() as i32))
}

/// 讓看不見的地方點得過去。
///
/// 這扇窗整片透明，但作業系統照整個 340×560 的矩形做命中判定，所以她頭頂
/// 上方那塊什麼都沒畫的區域會把點擊吃掉——底下的視窗收不到。`set_ignore_
/// cursor_events` 是整扇窗的開關，不是逐像素的，所以只能一直問「游標底下
/// 現在算不算實心」再翻那個開關。
///
/// 每一個「不知道」的出口都倒向維持可點：**寧可多擋住一點桌面，也不要讓她整
/// 個人點不到**。兩邊壞掉的代價不對稱：多擋住的那塊使用者看得見自己在點什麼，
/// 挪一下視窗就好；整個人點不到的話，畫面上明明有她、滑鼠卻穿過去，那看起來
/// 就是當掉了。她 `skipTaskbar`，所以工作列上也沒有東西可以點回來——系統匣的
/// 選單還在（見 `refresh_tray`），真要救救得回來，但那要使用者先想到「是這扇
/// 窗的問題」再去翻系統匣。那不是一條會有人自己走到的路。
///
/// 「游標現在在窗外」也算一種不知道，而且是最容易寫反的那一種——底層會照實說
/// 那個座標沒畫東西，可是我們要的不是那一刻的答案，是游標**進來的那一瞬間**該
/// 用哪一邊，而那一瞬間落在兩次輪詢之間。
///
/// 這條執行緒有**兩種**醒來的理由：睡飽了，或者 renderer 喊了一聲
/// （`pet_pointer_moved`）。喊的那一聲只決定這一拍**什麼時候**發生，不決定
/// 它算出什麼——醒來之後照樣自己去問 `cursor_position()`。renderer 送過來
/// 的座標會是第二份真相，而它和這裡量的是不同的東西（它沒有
/// `scale_factor()`，也不知道視窗被搬到哪）；更重要的是，讓它參與判斷就等於
/// 多一個翻開關的人，見 `pet_pointer_moved` 上面那段。
///
/// 決定本身一行都不在這裡：`visible`、游標位置和那份實心清單交給
/// [`bounds::hit::poll_step`]，這支函式只負責問得出那三樣、以及把答案交出去。
/// 這樣切是因為 `apps/desktop` 是另一個 workspace，根目錄的 `cargo test
/// --workspace` 走不到——留在這個檔案裡的判斷等於沒有人守。
fn spawn_click_through(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // Arc 先抄一份出來：`app.state()` 借來的東西不能跨過底下那個等待。
        let wake = app.state::<Shell>().hit_wake.clone();
        let mut last: Option<bool> = None;
        let mut nap = bounds::hit::POLL_NEAR_MS;
        loop {
            bounds::hit::wait_for_next_poll(&wake, nap);
            // 視窗沒了就收工。這是這條執行緒唯一的出口。
            let Some(win) = app.get_webview_window(PET) else {
                return;
            };
            // 看不見的時候不必問游標在哪——`poll_step` 那一臂本來就不看它。
            let visible = win.is_visible().unwrap_or(true);
            let here = if visible {
                cursor_in_window(&win)
            } else {
                None
            };
            let step = {
                let shell = app.state::<Shell>();
                let solid = shell.hit_solid.lock().expect("hit solid");
                // 鎖在這個區塊裡就還掉，不跨到下面那句 `set_ignore_cursor_events`。
                bounds::hit::poll_step(
                    Rect {
                        x: 0,
                        y: 0,
                        w: PET_W,
                        h: PET_H,
                    },
                    &solid,
                    visible,
                    here,
                )
            };
            nap = step.next_poll_ms;
            if last != Some(step.pass_through)
                && win.set_ignore_cursor_events(step.pass_through).is_ok()
            {
                last = Some(step.pass_through);
            }
        }
    });
}

#[cfg(test)]
mod click_through_tests {
    use super::*;

    /// `PET_W`／`PET_H` 從「開機時擺在哪」升級成命中判定的視窗矩形了，所以它
    /// 們現在必須真的等於視窗尺寸。
    ///
    /// 兩邊漂開不會有任何錯誤訊息。視窗變大而常數沒跟上，多出來的那一條永遠
    /// 算「窗外」＝永遠可點，她旁邊的空白又開始吃點擊——等於把這一版修好的事
    /// 悄悄還原回去。反過來則是把一塊不存在的區域設成穿透，沒有後果但也是假的。
    #[test]
    fn the_hit_test_rectangle_is_the_real_window_size() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let pet = conf["app"]["windows"]
            .as_array()
            .expect("windows")
            .iter()
            .find(|w| w["label"] == PET)
            .expect("pet window");
        assert_eq!(pet["width"].as_i64(), Some(i64::from(PET_W)), "寬");
        assert_eq!(pet["height"].as_i64(), Some(i64::from(PET_H)), "高");
        assert_eq!(
            pet["resizable"].as_bool(),
            Some(false),
            "能拉大小的話，一個常數就講不出視窗現在多大"
        );
    }
}

/// 把視窗現在的位置記進記憶體（不寫檔）。
///
/// 拖曳的時候 `Moved` 每幾毫秒就來一次，每次都寫硬碟是拿一個常駐程式去
/// 磨 SSD。寫檔的時機是收進系統匣、切換置頂、關閉——也就是**位置真的
/// 定下來**的那幾個時刻。代價寫在這裡：被工作管理員直接砍掉的話，
/// 那一次的移動記不住。
fn remember_position(win: &tauri::WebviewWindow, shell: &tauri::State<'_, Shell>) {
    if let Ok(pos) = win.outer_position() {
        let mut state = shell.state.lock().expect("pet state");
        state.x = pos.x;
        state.y = pos.y;
    }
}

fn monitors_of(win: &tauri::WebviewWindow) -> Vec<Rect> {
    win.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| Rect {
            x: m.position().x,
            y: m.position().y,
            w: m.size().width as i32,
            h: m.size().height as i32,
        })
        .collect()
}

/// 這一份記錄裡**沒有任何一個字來自螢幕**。
///
/// 只有這個殼自己的事：熱鍵搶到了沒、視窗開不開得起來、資料庫花了幾毫秒打開。
/// 之所以要寫成檔案，是因為 release build 沒有主控台（見檔案最上面的
/// `windows_subsystem`）——`tracing::error!("同意書開不起來：{e}")` 這種唯一
/// 講得出原因的話，在唯一會出貨的平台上是講給空氣聽的。
///
/// 這一條是從一張截圖上學到的：她卡住了，而我沒有任何辦法知道她卡在哪裡。
/// 一個讀不到的診斷，和沒有診斷是同一件事。
fn start_log(data_dir: Option<&PathBuf>) -> Option<std::fs::File> {
    start_log_at(data_dir?, "desktop.log")
}

/// 開一份新的記錄檔，並把上一輪的留成 `.1`。
///
/// 留一份是因為：當掉之後要看的正是**當掉那一輪**寫了什麼，而他為了找記錄檔
/// 一定得先把她重開——直接覆蓋的話，唯一有用的那一份就沒了。
fn start_log_at(dir: &std::path::Path, name: &str) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(name);
    let _ = std::fs::rename(&path, dir.join(format!("{name}.1")));
    std::fs::File::create(&path).ok()
}

fn initialize_logging(data_dir: Option<&PathBuf>) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "sister_desktop=info".into());
    match start_log(data_dir) {
        // 檔案裡不要 ANSI 跳脫碼，記事本打開會是一片亂碼。
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .init(),
        // 連資料目錄都問不出來就退回 stdout。開發時（有主控台）仍然看得到，
        // 出貨時看不到——但那個情況下她本來也幾乎做不了任何事。
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}

/// 只替取得 primary 身分的 desktop 開記錄檔，而且要早於 Tauri 建視窗。
///
/// 放在一般 `.setup` 會漏掉「WebView 本身建不起來」；放回 `main` 則第二個
/// instance 還沒被仲裁就會先把 primary 的 `desktop.log` 輪替掉。plugin 的註冊
/// 順序因此是 startup guard → 官方 single-instance receiver → logging →
/// 其他會碰產品狀態的 plugin。
fn primary_logging_plugin<R: tauri::Runtime>(
    data_dir: Option<PathBuf>,
) -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("primary-logging")
        .setup(move |_app, _api| {
            initialize_logging(data_dir.as_ref());
            tracing::info!("AI-Sister {} 起來了", env!("CARGO_PKG_VERSION"));
            Ok(())
        })
        .build()
}

#[cfg(windows)]
fn enter_product_lifecycle(
    launch_intent: LaunchIntent,
) -> sister_core::install_lifecycle::ProductLifecycleGuard {
    use sister_core::install_lifecycle::{InstallerAdmission, enter_product_lifecycle};
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONSTOP, MB_OK, MessageBoxW};
    use windows::core::w;

    let admission = match enter_product_lifecycle() {
        Ok(guard) => return guard,
        Err(admission) => admission,
    };

    // 登入啟動撞上安裝／移除時安靜退出；interactive 啟動則把「這次沒有進入產品狀態」
    // 說清楚。這裡早於 logging、Tauri plugin、WebView、DB 與 recorder admission。
    if launch_intent == LaunchIntent::Interactive {
        let body = match admission {
            InstallerAdmission::InProgress => {
                w!(
                    "偵測到 AI-Sister 安裝安全鎖。這次沒有進入產品功能，也沒有建立產品 log 或讀寫 AI-Sister 記憶／設定；請在相關操作結束後再試一次。"
                )
            }
            InstallerAdmission::Uncheckable => w!(
                "無法確認 AI-Sister 安裝安全鎖是否存在。為避免在程式檔可能變動時進入產品功能，這次沒有建立產品 log，也沒有讀寫 AI-Sister 記憶或設定。"
            ),
            InstallerAdmission::Clear => unreachable!("clear admission returned above"),
        };
        // SAFETY: 三個字串都是 static、NUL 結尾的 UTF-16 literal；沒有 owner window，
        // 因為這道 gate 刻意早於 Tauri 建立任何視窗。
        unsafe {
            let _ = MessageBoxW(None, body, w!("AI-Sister"), MB_OK | MB_ICONSTOP);
        }
    }
    std::process::exit(73);
}

fn main() {
    #[cfg(all(target_os = "macos", feature = "macos-ci-spike"))]
    match macos_ci::requested_directory() {
        Ok(Some(directory)) => {
            // 在 data dir、logging、WebView 與任何正常產品狀態之前走完。這個
            // feature-only 入口只負責維持一棵可由 CI 查證的 app/child process tree。
            if let Err(error) = macos_ci::run(&directory) {
                eprintln!("macOS app-tree diagnostic failed: {error}");
                std::process::exit(2);
            }
            return;
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("macOS app-tree diagnostic arguments are invalid: {error}");
            std::process::exit(2);
        }
    }

    #[cfg(windows)]
    let launch_intent = launch_intent(std::env::args_os().skip(1));
    #[cfg(not(windows))]
    let launch_intent = LaunchIntent::Interactive;

    #[cfg(windows)]
    let _product_lifecycle_guard = enter_product_lifecycle(launch_intent);

    let data_dir = sister_core::config::Config::default_data_dir();
    let state_path = data_dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join("pet-window.json");

    let builder = tauri::Builder::default();
    #[cfg(windows)]
    // guard 必須是第一個 plugin，官方 receiver 緊接在後。後來那個行程要在
    // 註冊熱鍵、setup 與任何產品狀態 mutation 之前退出；尤其不能建立一份
    // 新的 desktop.log 把原本那輪改名。
    let builder = builder
        .plugin(single_instance::startup_guard_plugin())
        .plugin(single_instance_plugin());

    builder
        .plugin(primary_logging_plugin(data_dir.clone()))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(Shell {
            state: Mutex::new(bounds::load(&state_path).unwrap_or_default()),
            state_path,
            data_dir,
            db: Mutex::new(None),
            recorder: Mutex::new(None),
            asset_operation: Arc::new(AtomicU8::new(ASSET_IDLE)),
            asset_cancel: Arc::new(AtomicBool::new(false)),
            azure_tts_in_flight: Arc::new(AtomicBool::new(false)),
            azure_tts_generation: Arc::new(AtomicU64::new(0)),
            azure_tts_active_generation: Arc::new(AtomicU64::new(
                AZURE_TTS_NO_ACTIVE_GENERATION,
            )),
            azure_tts_admission: Arc::new(Mutex::new(())),
            azure_tts_transition: Arc::new(Mutex::new(())),
            brain_cli_state: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            answer_cli: Mutex::new(None),
            diagnostics: Mutex::new(sister_core::diagnose::Notebook::new()),
            hit_solid: Mutex::new(Vec::new()),
            hit_wake: std::sync::Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        })
        .manage(Hotkey(Mutex::new(HotkeyView::default())))
        .invoke_handler(tauri::generate_handler![
            toggle_pin,
            pet_solid_set,
            pet_pointer_moved,
            diagnose_note,
            diagnose_export,
            hide_to_tray,
            ask,
            answer_cli_cancel,
            open_frame,
            log_click,
            mark_query,
            frame_image,
            pause_state,
            master_stop_state,
            master_stop_presentation_begin,
            master_stop_presentation_end,
            answer_local_speech_admit,
            persona_fixed_voice_admit,
            recording_state,
            start_recording,
            stop_recording,
            recorder_supervisor_state,
            recorder_log_tail,
            last_recording_end,
            has_ever_recorded,
            has_ever_stored,
            toggle_pause,
            persona_read,
            persona_voice_read,
            persona_asset_status,
            persona_asset_install,
            persona_asset_cancel,
            persona_asset_remove,
            persona_voice_set,
            azure_tts_read,
            azure_tts_config_set,
            azure_tts_key_set,
            azure_tts_key_delete,
            azure_tts_cancel,
            azure_tts_speak,
            login_startup_read,
            login_startup_set,
            platform_access_read,
            platform_access_open,
            settings_read,
            settings_write,
            brain_cli_read,
            brain_cli_connect,
            brain_cli_test,
            brain_cli_cancel,
            eval_report_view,
            lint_url_rules,
            privacy_health,
            open_settings,
            open_timeline,
            timeline_days,
            timeline_moments,
            timeline_chapters,
            timeline_merge_chapters,
            timeline_split_chapter,
            timeline_undo_segment_edit,
            memory_guesses,
            memory_current_guess,
            memory_commitments,
            memory_outbound,
            memory_day_summary,
            correct_l2,
            commitment_kill,
            commitment_other,
            forget_preview,
            forget_range,
            consent_read,
            consent_set,
            open_onboarding,
            hotkey_state,
            hotkey_set
            ,gatekeeper_check
            ,gatekeeper_react
            ,hands_execute
            ,url_policy_read
            ,url_policy_write
        ])
        .setup(move |app| {
            let win = app
                .get_webview_window(PET)
                .expect("pet window is declared in tauri.conf.json");
            let shell = app.state::<Shell>();

            // 先把唯一的 child owner 放進 managed state，才接 login intent。Windows
            // Run 的 primary 與稍早撞進 single-instance callback 的 secondary 都走
            // 同一條 channel；登入不顯示視窗，也不自動打開同意書。
            let recorder = recorder_supervisor::Handle::spawn(
                app.handle().clone(),
                shell.data_dir.clone(),
            );
            *shell.recorder.lock().expect("recorder supervisor") = Some(recorder.clone());
            let should_start_for_login = launch_intent == LaunchIntent::Login;
            #[cfg(windows)]
            let should_start_for_login = should_start_for_login
                || SECOND_INSTANCE_LOGIN_PENDING.swap(false, Ordering::AcqRel);
            if should_start_for_login
                && let Err(error) = recorder.login_start()
            {
                tracing::error!("Windows 登入啟動交不出去：{error}");
            }

            // ---- 位置 ----
            let screens = monitors_of(&win);
            let saved = *shell.state.lock().expect("pet state");
            let first_run = saved.x == 0 && saved.y == 0;

            let place = if first_run {
                let primary = win
                    .primary_monitor()
                    .ok()
                    .flatten()
                    .map(|m| Rect {
                        x: m.position().x,
                        y: m.position().y,
                        w: m.size().width as i32,
                        h: m.size().height as i32,
                    })
                    .unwrap_or(Rect {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                    });
                let (x, y) = bounds::first_run_corner(PET_W, PET_H, primary);
                Rect {
                    x,
                    y,
                    w: PET_W,
                    h: PET_H,
                }
            } else {
                // 這一行就是 TokenMonster 少掉的那一步：還原之前先問「現在
                // 還看得到嗎」。見 bounds.rs 的說明。
                bounds::nudge_onto(
                    Rect {
                        x: saved.x,
                        y: saved.y,
                        w: PET_W,
                        h: PET_H,
                    },
                    &screens,
                )
            };

            let _ = win.set_position(PhysicalPosition::new(place.x, place.y));
            let _ = win.set_always_on_top(saved.pinned);
            spawn_click_through(app.handle().clone());
            {
                let mut state = shell.state.lock().expect("pet state");
                state.x = place.x;
                state.y = place.y;
            }
            if launch_intent == LaunchIntent::Interactive {
                let _ = win.show();
            } else {
                tracing::info!("Windows 登入啟動：桌面姊妹留在系統匣，不彈視窗");
            }
            #[cfg(windows)]
            if let Some(outcome) =
                take_deferred_reveal(&SECOND_INSTANCE_REVEAL_PENDING, &win)
            {
                match outcome {
                    ExistingInstanceReveal::Revealed => {
                        tracing::info!("開機中的第二次啟動請求已補做：桌面姊妹已顯示並取得焦點")
                    }
                    ExistingInstanceReveal::ShowFailed(error) => {
                        tracing::error!("開機中的第二次啟動請求補做顯示失敗：{error}")
                    }
                    ExistingInstanceReveal::FocusFailed(error) => {
                        tracing::error!("開機中的第二次啟動請求已顯示，但取得焦點失敗：{error}")
                    }
                    // 傳進去的是剛從 Tauri 取出的實體視窗，不會走這一臂。
                    ExistingInstanceReveal::Missing => {
                        tracing::error!("開機中的第二次啟動請求補做時，主視窗仍不存在")
                    }
                }
            }

            // ---- 先去把資料庫打開 ----
            //
            // 不等人問問題才開。第一次開可能要跑 migration，而 003 要把整張
            // `text_chunks` 讀出來重算 bigram——升級上來的資料庫愈大愈久。那段
            // 時間如果是掛在「他剛剛按下 Enter」上，畫面就會停在「想一下…」，
            // 而他無從分辨那是她在想、還是她死了。
            //
            // 開不起來**不在這裡報錯**：她本來就該在一台什麼都還沒錄過的機器上
            // 站得住。真正要講的那句話（「還沒有任何記憶——先跑 sister record」）
            // 留給他真的問問題的時候講，那時候他才需要知道。
            let warm = app.handle().clone();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                match with_db(&warm.state::<Shell>(), |_| Ok(())) {
                    Ok(()) => {
                        tracing::info!("資料庫開好了（{} ms）", started.elapsed().as_millis());
                    }
                    Err(e) => tracing::info!("資料庫先不開：{e}"),
                }
            });

            // ---- 全域暫停熱鍵 ----
            //
            // 系統匣那一顆要先看得到圖示、再點開選單；熱鍵是**不用先找到她**的
            // 那條路，而「我現在不想被看」最常發生的時機，正是她不在畫面上、
            // 而且你手上正忙著別的事的時候。
            //
            // 搶不到不是致命的（系統匣那顆還在），所以這裡不 `?`——但也不能就
            // 這樣算了：狀態存進 `Hotkey`，設定頁上寫得出「這一組被別人拿走了」。
            //
            // **讀不出設定檔的時候不可以安靜地換一組。** 這裡以前是
            // `Config::load(&p).ok().unwrap_or_default()`——一個手寫壞掉的
            // `config.toml`（或被防毒／OneDrive 鎖著、磁碟滿）會讓他設的
            // `Ctrl+Alt+S` 變成內建的那一組。症狀是他按 S 沒有反應，而設定頁
            // 指著另一組說「搶到了」。這一頁上別的地方早就在對付這種事
            // （`setUnreadable`、`Config::reload` 的「繼續用舊的那一份」），
            // 只有這條路上的那個 `.ok()` 把原因整個吞掉了。
            {
                let loaded = config_path().and_then(|p| {
                    sister_core::config::Config::load(&p).map_err(|e| format!("{e:#}"))
                });
                let config_unreadable = loaded.as_ref().err().cloned();
                let shell_config = loaded.unwrap_or_default().shell;
                let wanted = shell_config.pause_shortcut;
                let hands_wanted = shell_config.hands_stop_shortcut;
                let view = HotkeyView {
                    config_unreadable,
                    ..apply_hotkey(app.handle(), &wanted, &hands_wanted)
                };
                if view.hands_collided {
                    tracing::warn!("熱鍵撞號：{} 留給拔手，暫停熱鍵已讓掉", view.hands_wanted);
                }
                match &view.hands_reason {
                    Some(reason) => tracing::warn!("拔手熱鍵 {} 註冊不起來：{reason}", view.hands_wanted),
                    None if view.hands_wanted.is_empty() => tracing::info!("拔手熱鍵是關掉的"),
                    None => tracing::info!("拔手熱鍵 {} 搶到了", view.hands_wanted),
                }
                // 成功也要留一行。「搶到了」和「這段程式根本沒跑到」在一份
                // 只記失敗的記錄檔裡長得一模一樣，而那正是他按了熱鍵沒反應時
                // 唯一想分辨的兩件事。
                // 判準是 `registered`，不是 `wanted.is_empty()`：`wanted` 現在裝的是
                // **設定檔裡那一組**（撞號時也照樣有值，這樣設定頁才講得出他設的是
                // 哪一顆）。拿空不空來問「有沒有搶到」的話，撞號那一輪會掉進最後
                // 一臂印「暫停熱鍵 Ctrl+Alt+H 搶到了」——而那顆這一輪一次都沒送去
                // 註冊，按下去做的是拔手。這幾行存在的理由就是要分辨「搶到了」和
                // 「這段程式根本沒跑到」，印肯定句給第三種情況正好毀掉那件事。
                match &view.reason {
                    Some(reason) => tracing::warn!("暫停熱鍵 {} 註冊不起來：{reason}", view.wanted),
                    None if view.hands_collided => {
                        tracing::info!("暫停熱鍵 {} 讓給拔手了，這一輪沒送去註冊", view.wanted)
                    }
                    None if !view.registered => tracing::info!("暫停熱鍵是關掉的"),
                    None => tracing::info!("暫停熱鍵 {} 搶到了", view.wanted),
                }
                *app.state::<Hotkey>().0.lock().expect("hotkey") = view;
            }

            // ---- 系統匣 ----
            // 暫停也放在系統匣，因為熱鍵可能被別的程式搶走，而她收起來的時候
            // 系統匣是最後一個一定按得到的地方。
            let paused_now = pause_state(app.state::<Shell>());
            let show_item = MenuItem::with_id(app, "show", "顯示 AI-Sister", true, None::<&str>)?;
            let pause_item =
                MenuItem::with_id(app, "pause", pause_label(paused_now), true, None::<&str>)?;
            let hands_labels = app.state::<Shell>().data_dir.as_ref()
                .map(|dir| sister_hands::kill_switch::tray_hands_labels(dir))
                .unwrap_or_else(sister_hands::kill_switch::tray_hands_unknown_labels);
            let hands_stop_item =
                MenuItem::with_id(app, "hands-stop", hands_labels.0, true, None::<&str>)?;
            let hands_resume_item =
                MenuItem::with_id(app, "hands-resume", hands_labels.1, true, None::<&str>)?;
            let master_labels = master_stop_labels(master_stop_phase(
                app.state::<Shell>().data_dir.as_deref(),
            ));
            let master_stop_item =
                MenuItem::with_id(app, "master-stop", master_labels.0, true, None::<&str>)?;
            let master_resume_item =
                MenuItem::with_id(app, "master-resume", master_labels.1, true, None::<&str>)?;
            // 開始／停止和暫停是兩件事，所以是兩顆。暫停是「先別看，但留在
            // 這裡」，停止是「今天到此為止」——把停止做成「一直暫停」會留下
            // 一個永遠在跑卻永遠不做事的行程，而他在工作管理員裡看得到它。
            //
            // 這兩行字問的是「按下去會發生什麼」，所以看的是**有沒有人佔著**
            // ——理由在 [`set_record_labels`] 上面。
            let presence_now = app
                .state::<Shell>()
                .data_dir
                .as_ref()
                .map(|dir| sister_core::heartbeat::presence(dir, sister_core::now_ms()))
                .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
            let supervisor_phase_now = Some(recorder.view().phase);
            let record_presentation =
                record_menu_presentation(supervisor_phase_now, presence_now);
            let record_item = MenuItem::with_id(
                app,
                "record",
                record_presentation.label,
                true,
                None::<&str>,
            )?;
            let timeline_item =
                MenuItem::with_id(app, "timeline", "她記得的每一天…", true, None::<&str>)?;
            let settings_item = MenuItem::with_id(app, "settings", "設定…", true, None::<&str>)?;
            let consent_item =
                MenuItem::with_id(app, "consent", "四張同意書…", true, None::<&str>)?;
            // 開發者入口預設不存在，不是放一顆灰掉的按鈕讓一般使用者猜。
            // 設定檔讀不懂時也維持隱藏；這個選項只加一扇工具頁，沒有理由在
            // 不確定時自行打開。改完設定要重開桌面殼，選單才會重建。
            let developer_mode = config_path()
                .and_then(|path| {
                    sister_core::config::Config::load(&path).map_err(|e| format!("{e:#}"))
                })
                .map(|config| config.shell.developer_mode)
                .unwrap_or(false);
            let metrics_item = if developer_mode {
                Some(MenuItem::with_id(
                    app,
                    "metrics",
                    "評測指標…",
                    true,
                    None::<&str>,
                )?)
            } else {
                None
            };
            let quit_item = MenuItem::with_id(
                app,
                "quit",
                quit_menu_label(supervisor_phase_now, presence_now),
                true,
                None::<&str>,
            )?;
            // 全停比暫停、停止 recorder、拔手都重；用前後兩條分隔線把它放在
            // 既有停止類項目之後的獨立區塊，不跟任何一顆排成看似等價的 toggle。
            let before_master_separator = PredefinedMenuItem::separator(app)?;
            let after_master_separator = PredefinedMenuItem::separator(app)?;
            let menu = match &metrics_item {
                Some(metrics_item) => Menu::with_items(
                    app,
                    &[
                        &show_item,
                        &record_item,
                        &pause_item,
                        &hands_stop_item,
                        &hands_resume_item,
                        &before_master_separator,
                        &master_stop_item,
                        &master_resume_item,
                        &after_master_separator,
                        &timeline_item,
                        &settings_item,
                        &consent_item,
                        metrics_item,
                        &quit_item,
                    ],
                )?,
                None => Menu::with_items(
                    app,
                    &[
                        &show_item,
                        &record_item,
                        &pause_item,
                        &hands_stop_item,
                        &hands_resume_item,
                        &before_master_separator,
                        &master_stop_item,
                        &master_resume_item,
                        &after_master_separator,
                        &timeline_item,
                        &settings_item,
                        &consent_item,
                        &quit_item,
                    ],
                )?,
            };
            app.manage(PauseItem(pause_item));
            app.manage(RecordItem {
                item: record_item,
                shown: Mutex::new(record_presentation),
            });
            app.manage(QuitItem(quit_item));
            app.manage(HandsStopItem(hands_stop_item));
            app.manage(HandsResumeItem(hands_resume_item));
            app.manage(MasterStopItem(master_stop_item));
            app.manage(MasterResumeItem(master_resume_item));

            TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("icon").clone())
                .tooltip("AI-Sister")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window(PET) {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "pause" => {
                        // 失敗要看得見。一顆按了什麼都沒發生的暫停鍵，
                        // 使用者只會以為自己按到了。
                        if let Err(e) = toggle_pause(app.clone(), app.state::<Shell>()) {
                            tracing::error!("暫停切換失敗：{e}");
                        }
                    }
                    "hands-stop" | "hands-resume" => {
                        let shell = app.state::<Shell>();
                        let resume = event.id.as_ref() == "hands-resume";
                        let changed = shell
                            .data_dir
                            .as_ref()
                            .ok_or((sister_hands::kill_switch::WhyNotWritten::NoDataDir, None))
                            .and_then(|dir| {
                                if resume {
                                    sister_hands::kill_switch::release(dir)
                                        .map(|_| ())
                                        .map_err(|e| (sister_hands::kill_switch::WhyNotWritten::CannotWrite, Some(e.to_string())))
                                } else {
                                    sister_hands::kill_switch::pull(dir, sister_core::now_ms())
                                        .map(|_| ())
                                        .map_err(|e| (sister_hands::kill_switch::WhyNotWritten::CannotWrite, Some(e.to_string())))
                                }
                            });
                        match changed {
                            Ok(()) => refresh_tray(app),
                            Err((why, os_error)) => {
                                tracing::error!("拔手開關切換失敗：{why:?}（{os_error:?}）");
                                if let Some(win) = app.get_webview_window(PET) {
                                    let _ = win.show();
                                    let _ = win.set_focus();
                                }
                                let _ = app.emit(
                                    "hands-pulled",
                                    sister_hands::kill_switch::tray_hands_failure_message(why),
                                );
                                refresh_tray(app);
                            }
                        }
                    }
                    "master-stop" | "master-resume" => {
                        dispatch_master_stop_menu(app, event.id.as_ref());
                    }
                    "record" => {
                        // 執行使用者實際看到的那行字所綁定的 action，不在 click
                        // 時重算另一份。尤其 stale「停止記錄」絕不能因 recorder
                        // 剛好先收工而反轉成 Start；stale Start 則仍會撞 occupancy
                        // 與 OS lease，安全地失敗。
                        let shell = app.state::<Shell>();
                        let action = app.state::<RecordItem>().action();
                        // Wait 回條仍讀 click 當下的 presence，只用來說原因；它
                        // 不得改動上面已綁定的 action。
                        let now = sister_core::now_ms();
                        let presence = shell
                            .data_dir
                            .as_ref()
                            .map(|dir| sister_core::heartbeat::presence(dir, now))
                            .unwrap_or(sister_core::heartbeat::Presence::NeverStarted);
                        let supervisor_phase = recorder_handle(shell.inner())
                            .ok()
                            .map(|handle| handle.view().phase);
                        let done = match action {
                            RecordMenuAction::Start => start_recording(shell.clone()),
                            RecordMenuAction::Stop => stop_recording(shell.clone()),
                            RecordMenuAction::Wait => {
                                let reason = match supervisor_phase {
                                    Some(recorder_supervisor::SupervisorPhase::Uncertain) => {
                                        "recorder 狀態不明；為避免重複錄製，沒有再開一個"
                                            .to_owned()
                                    }
                                    Some(recorder_supervisor::SupervisorPhase::Quitting) => {
                                        "AI-Sister 正在結束，不會再啟動 recorder".to_owned()
                                    }
                                    _ => sister_core::heartbeat::occupied_why_of(presence, now)
                                        .unwrap_or_else(|| {
                                            "recorder 正在轉換狀態，這一下沒有另開一個"
                                                .to_owned()
                                        }),
                                };
                                Err(reason)
                            }
                        };
                        match done {
                            // 立刻改字，不等下一次輪詢——按了之後那一顆要當場
                            // 看起來不一樣，不然他會再按一次。
                            Ok(()) => {
                                refresh_tray(app);
                            }
                            Err(e) => {
                                tracing::error!("開始／停止記錄失敗：{e}");
                                // 系統匣選單上沒有一格能放字。只寫進記錄檔的話，
                                // 按下去的後果是**什麼都沒發生**——而 `e` 是一句
                                // 寫好的完整中文（同意書沒簽、找不到 sister.exe、
                                // 已經有一個在跑），只是躺在他不會開的檔案裡。
                                //
                                // 先把視窗叫出來再送：收在系統匣的時候，那邊沒有
                                // 人在聽，這句話會掉在地上。
                                if let Some(win) = app.get_webview_window(PET) {
                                    let _ = win.show();
                                    let _ = win.set_focus();
                                }
                                let _ = app.emit("recorder-failed", e);
                            }
                        }
                    }
                    "timeline" => {
                        spawn_window(app.clone(), "時間軸", open_timeline_window);
                    }
                    "settings" => {
                        spawn_window(app.clone(), "設定頁", open_settings_window);
                    }
                    "consent" => {
                        spawn_window(app.clone(), "同意書", open_onboarding_window);
                    }
                    "metrics" => {
                        spawn_window(app.clone(), "評測指標", open_metrics_window);
                    }
                    "quit" => {
                        let shell = app.state::<Shell>();
                        // 走人之前把 recorder 也叫停。留著的話，他關掉的是唯一
                        // 看得見的那個視窗，而螢幕還在被記錄——他會以為自己
                        // 已經關掉了。這比「她其實沒在錄卻說在聽」更糟：那個是
                        // 少記了，這個是在他以為關掉之後繼續記。
                        //
                        // durable stop 也會讓終端機開的 recorder 收工；Child 落刀
                        // 則只碰這個 desktop 自己 spawn、且還沒開資料庫的那一個。
                        // worker 做完兩步才回來，並先讓所有 retry timer 失效。
                        if let Err(e) = quit_recorder(shell.inner()) {
                            tracing::error!("結束時停不掉 recorder；desktop 留著：{e}");
                            if let Some(win) = app.get_webview_window(PET) {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                            let _ = app.emit("recorder-failed", e);
                            refresh_tray(app);
                            return;
                        }
                        shell.persist();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左鍵點圖示 = 開關。這是這類常駐程式唯一大家都會試的手勢。
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let pet_window = tray.app_handle().get_webview_window(PET);
                        if let Some(win) = pet_window {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // ---- 系統匣的字自己刷 ----
            //
            // 理由寫在 `refresh_tray`：那三行字以前是搭畫面輪詢的便車更新的，
            // 而畫面收進系統匣就不問了——正好是系統匣變成唯一介面的那一刻。
            let ticker = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(TRAY_REFRESH);
                    // 選單是 UI，回主執行緒去動。這一步失敗只有一個原因：事件
                    // 迴圈已經走了。那就跟著收工，不要留一條對著死掉的 app
                    // handle 每 5 秒喊一次的執行緒。
                    let on_main = ticker.clone();
                    if ticker
                        .run_on_main_thread(move || refresh_tray(&on_main))
                        .is_err()
                    {
                        break;
                    }
                }
            });

            // ---- 關閉 = 收起來，不是結束 ----
            let handle = app.handle().clone();
            win.on_window_event(move |event| match event {
                WindowEvent::CloseRequested { api, .. } => {
                    // Alt+F4 在這裡不該是「再見」。真的要結束走系統匣選單。
                    api.prevent_close();
                    if let Some(win) = handle.get_webview_window(PET) {
                        let shell = handle.state::<Shell>();
                        remember_position(&win, &shell);
                        shell.persist();
                        let _ = win.hide();
                    }
                }
                WindowEvent::Moved(pos) => {
                    let shell = handle.state::<Shell>();
                    let mut state = shell.state.lock().expect("pet state");
                    state.x = pos.x;
                    state.y = pos.y;
                }
                WindowEvent::Destroyed => {
                    // Renderer 已不存在，不可能再 commit 晚回覆；把 Begun lease 一次
                    // 放掉。一般 Alt+F4 只 hide、會被上面的 prevent_close 攔住。
                    clear_presentations();
                }
                _ => {}
            });

            // 還沒回答的同意書由主視窗逐張問；不再另外彈一扇視窗，讓使用者
            // 打完「同意／不同意」後原本的問題能在同一條對話裡繼續。設定頁與
            // 系統匣仍保留完整四張卡片，供日後查看、重簽與撤回。

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("build AI-Sister")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let shell = app.state::<Shell>();
                // 系統匣以外的退出（Windows session shutdown 等）也先讓 retry
                // 失效並留下 durable stop。tray quit 已做過時，第二次只會快速
                // 看見 handle 已拿掉，不會再送第二次。若 stop 寫不進去，則讓
                // desktop 留著並說明原因；不能讓唯一可見 UI 消失而 recorder 繼續。
                if let Err(error) = quit_recorder(shell.inner()) {
                    api.prevent_exit();
                    tracing::error!("退出已攔下，因為 recorder 停止意圖寫不進去：{error}");
                    if let Some(win) = app.get_webview_window(PET) {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                    let _ = app.emit("recorder-failed", error);
                    return;
                }
                // 到這裡才確定沒有 prevent_exit；renderer 不會再有機會送
                // presentation end。若上面的 durable stop 寫不進去而留在 app，
                // 提早 clear 會放掉仍可能繼續 Promise／playback 的 activity guard。
                clear_presentations();
                shell.persist();
            }
        });
}
