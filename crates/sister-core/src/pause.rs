//! 暫停控制狀態：兩個行程之間唯一的那條線。
//!
//! `sister.exe record` 在錄，`sister-desktop.exe` 上有一顆暫停鍵，兩者是**不同
//! 的行程**——沒有常駐服務、沒有 port、沒有網路 IPC。`paused.flag` 仍是舊版也
//! 看得懂的 fail-closed current signal；`pause.state` 留住已解除的 generation，
//! `pause.lock` 則把兩個行程的 read/commit 排成唯一順序。這是一個三檔
//! 協定，三個檔案都只在 data dir；不能只刪其中一個來「修復」狀態。
//!
//! 三條規則，每一條都是隱私承諾的一部分：
//!
//! 1. **不確定就是暫停。** 讀不到、壞掉、權限不足——一律當成暫停。反過來那個
//!    版本（「看不出來，那就繼續錄」）正是會讓「我按了暫停」變成一句空話的
//!    失效模式。
//! 2. **不會自己過期。** 桌面程式當掉時如果是暫停狀態，它就一直暫停到有人明確
//!    解除為止。會自己醒來的暫停等於沒有暫停。
//! 3. **暫停要留下紀錄。** 旗標只管「現在」；「那三個小時她沒在看」由 recorder
//!    寫成 `CapturePaused` / `CaptureResumed` 兩筆事件。少了那兩筆，資料裡的
//!    空洞跟「那段時間什麼都沒發生」長得一模一樣。
//!
//! Flag 內容保留暫停時戳給 `doctor` 講人話；判定仍以存在為先。內容空白／舊版
//! 純 timestamp 都是合法的 paused；state 或 lock 壞掉則明確是 Indeterminate，
//! 不能拿 `generation = 0` 冒充從未暫停。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::dir_state::{DirState, dir_state};
use crate::model::Millis;

/// 她現在的暫停狀態，以及判成暫停的理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseState {
    /// 沒有暫停。
    Recording,
    /// 暫停中，而且知道從什麼時候起。
    Since(Millis),
    /// 暫停中：旗標檔確定在，但內容沒有可信的時間。請用
    /// `sister resume` 讓完整的三檔協定一起恢復，不要手動刪旗標。
    FlagPresentButUnreadable,
    /// 暫停中：data dir 是正常目錄，但旗標檔本身讀不到。
    FlagUncheckable,
    /// 暫停中：連 data dir 都讀不到，所以刻意一律當成暫停。
    /// 旗標在不在沒有看過，刪它不一定有用。
    PathUnreadable,
    /// 暫停中：跨行程 pause state／lock 讀不到或內容損毀。
    ///
    /// 這不能冒充上面的 `FlagPresentButUnreadable`：那句話聲稱已確認
    /// `paused.flag` 存在，而這一格連完整控制狀態都沒有確認。
    ControlStateUncheckable,
}

/// 旗標檔名。放在 data dir 裡，跟 `sister.db` 同一層。
const FLAG: &str = "paused.flag";
/// 最近一個 pause generation；解除暫停後仍然保留。
const STATE: &str = "pause.state";
/// 跨行程 read/write transaction 的鎖。檔案本身永不刪除；owner crash 時
/// 作業系統會釋放 handle lock，不會留下 stale lock。
const LOCK: &str = "pause.lock";
const STATE_VERSION: u32 = 1;

/// 一次明確 pause request 的世代。
///
/// 數值只用來比較是否換代；呼叫端不能自行建構或把 `0` 當成一次 pause。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PauseGeneration(u64);

impl PauseGeneration {
    const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 最近一段 pause 的持久化邊界。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PauseEpoch {
    generation: PauseGeneration,
    paused_at: Option<Millis>,
    resumed_at: Option<Millis>,
}

impl PauseEpoch {
    const INITIAL: Self = Self {
        generation: PauseGeneration::INITIAL,
        paused_at: None,
        resumed_at: None,
    };

    pub const fn generation(self) -> PauseGeneration {
        self.generation
    }

    /// `None` 只會來自沒有歷史的初始狀態，或沒有可解析時間的 legacy flag。
    pub const fn paused_at(self) -> Option<Millis> {
        self.paused_at
    }

    /// 目前仍暫停時是 `None`；已完整解除時保留真正的 resume request 時間。
    pub const fn resumed_at(self) -> Option<Millis> {
        self.resumed_at
    }
}

/// 一次帶 generation 的跨行程 pause 快照。
///
/// `Indeterminate` 本身就是 fail-closed 暫停，不是「沒有量到所以 generation=0」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseSnapshot {
    Recording(PauseEpoch),
    Paused(PauseEpoch),
    Indeterminate,
}

impl PauseSnapshot {
    pub const fn is_paused(self) -> bool {
        !matches!(self, Self::Recording(_))
    }

    pub const fn epoch(self) -> Option<PauseEpoch> {
        match self {
            Self::Recording(epoch) | Self::Paused(epoch) => Some(epoch),
            Self::Indeterminate => None,
        }
    }
}

/// 持有 shared pause lock 的快照。
///
/// Recorder 在最後一次重驗後把這個值留到 PNG／DB transaction 完成；pause
/// writer 要取得 exclusive lock，因此「寫入在 pause 前」或「pause 在寫入前」
/// 有唯一順序，不再剩 probe→commit 的 TOCTOU。
pub struct PauseSnapshotGuard {
    snapshot: PauseSnapshot,
    _lock: Option<File>,
}

impl PauseSnapshotGuard {
    pub const fn snapshot(&self) -> PauseSnapshot {
        self.snapshot
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedPauseState {
    version: u32,
    generation: u64,
    paused: bool,
    paused_at: Option<Millis>,
    resumed_at: Option<Millis>,
}

impl PersistedPauseState {
    fn validate(self) -> Result<Self> {
        anyhow::ensure!(
            self.version == STATE_VERSION,
            "unsupported pause state version {}",
            self.version
        );
        anyhow::ensure!(self.generation > 0, "pause generation must be nonzero");
        anyhow::ensure!(
            !self.paused || self.resumed_at.is_none(),
            "paused state cannot already have a resume timestamp"
        );
        anyhow::ensure!(
            self.paused || self.resumed_at.is_some(),
            "recording state with pause history needs a resume timestamp"
        );
        Ok(self)
    }

    const fn epoch(self) -> PauseEpoch {
        PauseEpoch {
            generation: PauseGeneration(self.generation),
            paused_at: self.paused_at,
            resumed_at: self.resumed_at,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum StateRecord {
    Missing,
    Present(PersistedPauseState),
}

#[derive(Debug, Clone, Copy)]
enum FlagRecord {
    Missing,
    /// alpha.103+ 格式。`paused_at=None` 是由空／損毀 legacy flag 安全遷移而來。
    Versioned {
        generation: u64,
        paused_at: Option<Millis>,
    },
    /// 舊版只寫 timestamp；空檔或垃圾仍代表 paused，只是起點不知道。
    Legacy {
        paused_at: Option<Millis>,
    },
}

pub fn flag_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FLAG)
}

fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE)
}

fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCK)
}

/// `child` 是 `data_dir/paused.flag` 的 `try_exists` 答案；錯誤內容在這一步不重要。
fn decide(child: Result<bool, ()>, dir: DirState) -> bool {
    match child {
        Ok(true) | Err(()) => true,
        Ok(false) => match dir {
            DirState::Dir | DirState::Absent => false,
            DirState::NotADir | DirState::Unreadable => true,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PauseDecision {
    Recording,
    FlagPresent,
    FlagUncheckable,
    PathUnreadable,
}

fn decide_for(data_dir: &Path, child: Result<bool, ()>) -> PauseDecision {
    let dir = dir_state(data_dir);
    if !decide(child, dir) {
        return PauseDecision::Recording;
    }
    match child {
        Ok(true) => PauseDecision::FlagPresent,
        Err(()) if dir == DirState::Dir => PauseDecision::FlagUncheckable,
        Ok(false) | Err(()) => PauseDecision::PathUnreadable,
    }
}

/// 她現在是不是閉著眼睛。
///
/// 回傳 `bool` 而不是 `Result<bool>` 是刻意的：這個問題只有一個安全的預設答案，
/// 而把錯誤丟給呼叫端，等於讓每一個呼叫端各自決定一次「讀不到的時候要不要繼續
/// 錄」——只要有一個人答錯，承諾就破了。所以答案在這裡就定死。
///
/// **定死的那個答案，在 alpha.75 之前於 Windows 上是錯的。** 舊的寫法是
/// `flag_path(data_dir).try_exists().unwrap_or(true)`，而 Windows 會把
/// 「父路徑是檔案」的子查詢回成 `Ok(false)`，不是 Linux 的 `Err`——於是
/// 「我讀不到」被講成「旗標確定不在」，使用者按了暫停，她繼續錄。
///
/// 跟 `sister-hands` 的 `kill_switch::is_pulled` 同一個病、同一個修法，兩邊
/// 刻意各留一份（兩個 crate 不能形成正常相依）。**下面這一行同樣沒有 Linux
/// 測試守得住**：child 查詢要穿過 data dir，Linux 上它一定先回 `Err`，改回
/// 舊寫法照樣全綠。守住它的是 `unreadable_path_is_paused_fail_closed`，
/// 而那條只有在 Windows CI 上才走得到那一格。
pub fn is_paused(data_dir: &Path) -> bool {
    snapshot(data_dir).is_paused()
}

/// 她現在的暫停狀態。
///
/// 判定和 [`is_paused`] 共用 [`snapshot`]，避免 persistent generation 損毀時
/// 一邊說 Recording、另一邊卻正確 fail closed。
pub fn state(data_dir: &Path) -> PauseState {
    match snapshot(data_dir) {
        PauseSnapshot::Recording(_) => PauseState::Recording,
        PauseSnapshot::Paused(epoch) => epoch
            .paused_at()
            .map(PauseState::Since)
            .unwrap_or(PauseState::FlagPresentButUnreadable),
        PauseSnapshot::Indeterminate => {
            // Do not let a readable paused.flag hide a corrupt generation record. `resume`
            // validates pause.state before deleting the flag, so reporting an ordinary
            // `Since` here would promise a recovery path that is guaranteed to fail.
            match dir_state(data_dir) {
                DirState::NotADir | DirState::Unreadable => PauseState::PathUnreadable,
                DirState::Absent => PauseState::ControlStateUncheckable,
                DirState::Dir => {
                    if read_state_unlocked(data_dir).is_err() {
                        return PauseState::ControlStateUncheckable;
                    }
                    match read_flag_unlocked(data_dir) {
                        Ok(_) => PauseState::ControlStateUncheckable,
                        Err(_) => {
                            match decide_for(
                                data_dir,
                                flag_path(data_dir).try_exists().map_err(|_| ()),
                            ) {
                                PauseDecision::FlagPresent => PauseState::FlagPresentButUnreadable,
                                PauseDecision::FlagUncheckable => PauseState::FlagUncheckable,
                                PauseDecision::PathUnreadable => PauseState::PathUnreadable,
                                PauseDecision::Recording => PauseState::ControlStateUncheckable,
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 從什麼時候開始暫停的。純顯示用；`None` 可能是 legacy flag 沒有可解析
/// 時間、控制狀態無法確認，或目前確定正在錄。要分辨請用 [`state`]。
pub fn paused_since(data_dir: &Path) -> Option<Millis> {
    match snapshot(data_dir) {
        PauseSnapshot::Paused(epoch) => epoch.paused_at(),
        PauseSnapshot::Recording(_) => None,
        PauseSnapshot::Indeterminate => raw_paused_since(data_dir),
    }
}

fn raw_paused_since(data_dir: &Path) -> Option<Millis> {
    match parse_flag(&std::fs::read_to_string(flag_path(data_dir)).ok()?) {
        FlagRecord::Versioned { paused_at, .. } | FlagRecord::Legacy { paused_at } => paused_at,
        FlagRecord::Missing => None,
    }
}

/// 取得一份短生命週期快照。需要把「最後一次重驗」與後續持久化綁成同一個
/// 線性化區間時，請改用 [`snapshot_guard`] 並把 guard 留到 commit 結束。
pub fn snapshot(data_dir: &Path) -> PauseSnapshot {
    // Most callers only need a momentary answer. Keep that read side-effect free: status,
    // doctor, and answer paths must not create a data directory merely by inspecting it.
    //
    // A pre-alpha.103 directory may legitimately have state/flag but no pause.lock yet.
    // Read it once, then check the lock path again. If a new writer created the lock while
    // we were reading, take its shared lock and re-read; if the lock is still absent, this
    // snapshot linearizes immediately before any future writer creates it.
    match snapshot_guard_existing(data_dir) {
        Ok(Some(guard)) => return guard.snapshot(),
        Ok(None) => {}
        Err(_) => return PauseSnapshot::Indeterminate,
    }

    let observed = match dir_state(data_dir) {
        DirState::Absent => PauseSnapshot::Recording(PauseEpoch::INITIAL),
        DirState::Dir => read_snapshot_unlocked(data_dir).unwrap_or(PauseSnapshot::Indeterminate),
        DirState::NotADir | DirState::Unreadable => PauseSnapshot::Indeterminate,
    };

    match snapshot_guard_existing(data_dir) {
        Ok(Some(guard)) => guard.snapshot(),
        Ok(None) => observed,
        Err(_) => PauseSnapshot::Indeterminate,
    }
}

/// 取得快照並持有 shared 跨行程鎖。
///
/// 這支不回 `Result` 是刻意的：開目錄、開鎖、上鎖、讀檔、解析任一步失敗，
/// 都只有一個安全答案 [`PauseSnapshot::Indeterminate`]。該答案的 guard 不保證
/// 持有鎖，但呼叫端本來就不准在它之後寫內容。
pub fn snapshot_guard(data_dir: &Path) -> PauseSnapshotGuard {
    let file = match open_lock_file(data_dir) {
        Ok(file) => file,
        Err(_) => {
            return PauseSnapshotGuard {
                snapshot: PauseSnapshot::Indeterminate,
                _lock: None,
            };
        }
    };
    if fs4::FileExt::lock_shared(&file).is_err() {
        return PauseSnapshotGuard {
            snapshot: PauseSnapshot::Indeterminate,
            _lock: None,
        };
    }
    let observed = read_snapshot_unlocked(data_dir).unwrap_or(PauseSnapshot::Indeterminate);
    PauseSnapshotGuard {
        snapshot: observed,
        _lock: Some(file),
    }
}

fn snapshot_guard_existing(data_dir: &Path) -> Result<Option<PauseSnapshotGuard>> {
    let Some(file) = open_existing_lock_file(data_dir)? else {
        return Ok(None);
    };
    fs4::FileExt::lock_shared(&file).context("取得 pause read lock 失敗")?;
    let observed = read_snapshot_unlocked(data_dir)?;
    Ok(Some(PauseSnapshotGuard {
        snapshot: observed,
        _lock: Some(file),
    }))
}

fn read_snapshot_unlocked(data_dir: &Path) -> Result<PauseSnapshot> {
    let state = read_state_unlocked(data_dir)?;
    let flag = read_flag_unlocked(data_dir)?;

    match flag {
        FlagRecord::Missing => Ok(match state {
            StateRecord::Missing => PauseSnapshot::Recording(PauseEpoch::INITIAL),
            StateRecord::Present(state) if state.paused => PauseSnapshot::Paused(state.epoch()),
            StateRecord::Present(state) => PauseSnapshot::Recording(state.epoch()),
        }),
        FlagRecord::Legacy { paused_at } => {
            // Legacy flag 沒有 generation。若新版 state 已知仍 paused，就沿用它；
            // 否則先回舊 generation + Paused。解除時 writer 會在刪 flag **之前**
            // 配置下一代，所以完整 pause→resume 仍不會 ABA 消失。
            let epoch = match state {
                StateRecord::Present(state) if state.paused => state.epoch(),
                StateRecord::Present(state) => PauseEpoch {
                    generation: PauseGeneration(state.generation),
                    paused_at,
                    resumed_at: None,
                },
                StateRecord::Missing => PauseEpoch {
                    generation: PauseGeneration::INITIAL,
                    paused_at,
                    resumed_at: None,
                },
            };
            Ok(PauseSnapshot::Paused(epoch))
        }
        FlagRecord::Versioned {
            generation,
            paused_at,
        } => {
            anyhow::ensure!(generation > 0, "versioned pause flag generation is zero");
            let epoch = match state {
                StateRecord::Missing => PauseEpoch {
                    generation: PauseGeneration(generation),
                    paused_at,
                    resumed_at: None,
                },
                StateRecord::Present(state) if state.generation == generation => {
                    if let (Some(from_flag), Some(from_state)) = (paused_at, state.paused_at) {
                        anyhow::ensure!(
                            from_flag == from_state,
                            "pause flag/state timestamps disagree"
                        );
                    }
                    PauseEpoch {
                        generation: PauseGeneration(generation),
                        paused_at: paused_at.or(state.paused_at),
                        // state=Recording + flag present is a crash between the two resume
                        // writes. It remains effectively paused, but keeping resumed_at lets
                        // the retry preserve the original boundary instead of moving it.
                        resumed_at: state.resumed_at,
                    }
                }
                StateRecord::Present(state)
                    if !state.paused && state.generation.checked_add(1) == Some(generation) =>
                {
                    // Crash after publishing the new flag but before pause.state.
                    PauseEpoch {
                        generation: PauseGeneration(generation),
                        paused_at,
                        resumed_at: None,
                    }
                }
                StateRecord::Present(state) => anyhow::bail!(
                    "pause flag generation {generation} is inconsistent with state generation {}",
                    state.generation
                ),
            };
            Ok(PauseSnapshot::Paused(epoch))
        }
    }
}

fn read_state_unlocked(data_dir: &Path) -> Result<StateRecord> {
    let path = state_path(data_dir);
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StateRecord::Missing);
        }
        Err(error) => return Err(error).with_context(|| format!("讀取 {} 失敗", path.display())),
    };
    let state: PersistedPauseState =
        serde_json::from_str(&body).with_context(|| format!("解析 {} 失敗", path.display()))?;
    Ok(StateRecord::Present(state.validate()?))
}

fn read_flag_unlocked(data_dir: &Path) -> Result<FlagRecord> {
    let path = flag_path(data_dir);
    match path.try_exists() {
        Ok(false) => Ok(FlagRecord::Missing),
        Ok(true) => {
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("讀取 {} 失敗", path.display()))?;
            Ok(parse_flag(&body))
        }
        Err(error) => Err(error).with_context(|| format!("檢查 {} 失敗", path.display())),
    }
}

fn parse_flag(body: &str) -> FlagRecord {
    let trimmed = body.trim();
    let mut fields = trimmed.split_whitespace();
    if fields.next() == Some("v1") {
        let generation = fields.next().and_then(|field| field.parse::<u64>().ok());
        let paused_at = match fields.next() {
            Some("-") => Some(None),
            Some(field) => field.parse::<Millis>().ok().map(Some),
            None => None,
        };
        if let (Some(generation), Some(paused_at), None) = (generation, paused_at, fields.next()) {
            return FlagRecord::Versioned {
                generation,
                paused_at,
            };
        }
        // 部分寫入或未來格式仍以「flag 在＝paused」處理；不能讓 parse error
        // 變成 Recording。Writer 解除時會先為它配置新 generation。
        return FlagRecord::Legacy { paused_at: None };
    }
    FlagRecord::Legacy {
        paused_at: trimmed.parse().ok(),
    }
}

fn open_lock_file(data_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("建立 {} 失敗", data_dir.display()))?;
    let path = lock_path(data_dir);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("開啟 {} 失敗", path.display()))?;
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("檢查 {} 失敗", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "pause lock is not a regular file: {}",
        path.display()
    );
    Ok(file)
}

fn open_existing_lock_file(data_dir: &Path) -> Result<Option<File>> {
    let path = lock_path(data_dir);
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("開啟 {} 失敗", path.display()));
        }
    };
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("檢查 {} 失敗", path.display()))?;
    anyhow::ensure!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "pause lock is not a regular file: {}",
        path.display()
    );
    Ok(Some(file))
}

fn next_generation(state: StateRecord) -> Result<u64> {
    match state {
        StateRecord::Missing => Ok(1),
        StateRecord::Present(state) => state
            .generation
            .checked_add(1)
            .context("pause generation exhausted"),
    }
}

fn resolve_paused_epoch(
    state: StateRecord,
    flag: FlagRecord,
    requested_at: Millis,
) -> Result<PauseEpoch> {
    match flag {
        FlagRecord::Versioned {
            generation,
            paused_at,
        } => {
            anyhow::ensure!(generation > 0, "versioned pause flag generation is zero");
            match state {
                StateRecord::Missing => Ok(PauseEpoch {
                    generation: PauseGeneration(generation),
                    paused_at,
                    resumed_at: None,
                }),
                StateRecord::Present(current) if current.generation == generation => {
                    if let (Some(from_flag), Some(from_state)) = (paused_at, current.paused_at) {
                        anyhow::ensure!(
                            from_flag == from_state,
                            "pause flag/state timestamps disagree"
                        );
                    }
                    Ok(PauseEpoch {
                        generation: PauseGeneration(generation),
                        paused_at: paused_at.or(current.paused_at),
                        resumed_at: current.resumed_at,
                    })
                }
                StateRecord::Present(current)
                    if !current.paused && current.generation.checked_add(1) == Some(generation) =>
                {
                    Ok(PauseEpoch {
                        generation: PauseGeneration(generation),
                        paused_at,
                        resumed_at: None,
                    })
                }
                StateRecord::Present(current) => anyhow::bail!(
                    "pause flag generation {generation} is inconsistent with state generation {}",
                    current.generation
                ),
            }
        }
        FlagRecord::Legacy { paused_at } => match state {
            StateRecord::Present(current) if current.paused => Ok(current.epoch()),
            _ => Ok(PauseEpoch {
                generation: PauseGeneration(next_generation(state)?),
                paused_at,
                resumed_at: None,
            }),
        },
        FlagRecord::Missing => match state {
            StateRecord::Present(current) if current.paused => Ok(current.epoch()),
            _ => Ok(PauseEpoch {
                generation: PauseGeneration(next_generation(state)?),
                paused_at: Some(requested_at),
                resumed_at: None,
            }),
        },
    }
}

fn write_flag_atomic(data_dir: &Path, epoch: PauseEpoch) -> Result<()> {
    let at = epoch
        .paused_at
        .map_or_else(|| "-".to_string(), |at| at.to_string());
    atomic_write(
        data_dir,
        &flag_path(data_dir),
        format!("v1 {} {at}", epoch.generation.get()).as_bytes(),
    )
}

fn write_state_atomic(data_dir: &Path, state: PersistedPauseState) -> Result<()> {
    let body = serde_json::to_vec(&state).context("serialize pause state")?;
    atomic_write(data_dir, &state_path(data_dir), &body)
}

fn atomic_write(data_dir: &Path, destination: &Path, body: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut opened = None;
    let mut temp_path = None;
    for _ in 0..128 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = data_dir.join(format!(".pause-tmp-{}-{serial}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                opened = Some(file);
                temp_path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("建立 pause 暫存檔於 {} 失敗", data_dir.display()));
            }
        }
    }
    let mut file = opened.context("找不到可用的 pause 暫存檔名")?;
    let temp = temp_path.expect("temp path accompanies opened file");
    let result = (|| -> Result<()> {
        file.write_all(body)
            .with_context(|| format!("寫入 {} 失敗", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("同步 {} 失敗", temp.display()))?;
        drop(file);
        replace_file_atomically(&temp, destination)?;
        #[cfg(unix)]
        File::open(data_dir)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("同步 {} 目錄失敗", data_dir.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination).with_context(|| {
        format!(
            "原子替換 {} → {} 失敗",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::core::PCWSTR;

    // `std::fs::rename` does not replace an existing destination on Windows. Both
    // paused.flag and pause.state are deliberately rewritten in place, so the second
    // transition would otherwise fail forever. MoveFileExW gives the same-directory
    // atomic replacement we need without an unsafe remove-then-rename window.
    let source_wide: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .with_context(|| {
        format!(
            "原子替換 {} → {} 失敗",
            source.display(),
            destination.display()
        )
    })
}

fn set_paused_unlocked(data_dir: &Path, paused: bool, ts: Millis) -> Result<()> {
    let state = read_state_unlocked(data_dir)?;
    let flag = read_flag_unlocked(data_dir)?;
    let currently_paused = !matches!(flag, FlagRecord::Missing)
        || matches!(state, StateRecord::Present(current) if current.paused);

    if paused {
        let mut epoch = resolve_paused_epoch(state, flag, ts)?;
        epoch.resumed_at = None;
        // Flag first: a crash can leave an extra pause, never a missing pause.
        write_flag_atomic(data_dir, epoch)?;
        write_state_atomic(
            data_dir,
            PersistedPauseState {
                version: STATE_VERSION,
                generation: epoch.generation.get(),
                paused: true,
                paused_at: epoch.paused_at,
                resumed_at: None,
            },
        )?;
        return Ok(());
    }

    if !currently_paused {
        return Ok(());
    }

    let epoch = resolve_paused_epoch(state, flag, ts)?;
    let prior_resume = match state {
        StateRecord::Present(current)
            if !current.paused && current.generation == epoch.generation.get() =>
        {
            current.resumed_at
        }
        _ => None,
    };
    // State first, flag last. If the process dies in between, legacy and new readers both
    // still see the flag and remain paused; a retry recognizes the same generation.
    write_state_atomic(
        data_dir,
        PersistedPauseState {
            version: STATE_VERSION,
            generation: epoch.generation.get(),
            paused: false,
            paused_at: epoch.paused_at,
            resumed_at: prior_resume.or(Some(ts)),
        },
    )?;
    match std::fs::remove_file(flag_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("刪除 paused.flag 失敗"),
    }
}

fn set_paused_fail_closed_unlocked(data_dir: &Path, paused: bool, ts: Millis) -> Result<()> {
    match set_paused_unlocked(data_dir, paused, ts) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Even an exhausted/corrupt generation must not turn a click on Pause into
            // continued recording. A legacy timestamp flag has no generation, but presence
            // is sufficient to stop every old and new reader. The command still returns the
            // original error; it must not announce that the full transaction succeeded.
            if paused && matches!(flag_path(data_dir).try_exists(), Ok(false)) {
                atomic_write(
                    data_dir,
                    &flag_path(data_dir),
                    ts.to_string().as_bytes(),
                )
                .with_context(|| {
                    format!(
                        "pause state transaction failed ({error:#}); fail-closed flag also failed"
                    )
                })?;
            }
            Err(error)
        }
    }
}

/// 按下暫停／解除暫停。
///
/// 兩個方向都是幂等的：已經暫停了再按暫停不會動到原本的時戳（不然「暫停多久
/// 了」會被每一次重按洗掉），已經在錄了再按解除也不算錯誤。
pub fn set_paused(data_dir: &Path, paused: bool, ts: Millis) -> Result<()> {
    let lock = open_lock_file(data_dir)?;
    fs4::FileExt::lock(&lock).context("取得 pause write lock 失敗")?;
    set_paused_fail_closed_unlocked(data_dir, paused, ts)
}

/// 在同一把跨行程 exclusive lock 裡讀取並翻轉狀態。
///
/// Desktop 不能先 [`is_paused`] 再 [`set_paused`]：兩個同時到達的 hotkey 都會
/// 從同一個舊值算出相同答案，兩次 toggle 只翻一次。這支函式回傳翻轉後是否暫停。
pub fn toggle_paused(data_dir: &Path, ts: Millis) -> Result<bool> {
    let lock = open_lock_file(data_dir)?;
    fs4::FileExt::lock(&lock).context("取得 pause toggle lock 失敗")?;
    let current = read_snapshot_unlocked(data_dir)?;
    let paused = match current {
        PauseSnapshot::Recording(_) => true,
        PauseSnapshot::Paused(_) => false,
        PauseSnapshot::Indeterminate => {
            anyhow::bail!("pause state is indeterminate; refusing to toggle")
        }
    };
    set_paused_fail_closed_unlocked(data_dir, paused, ts)?;
    Ok(paused)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier};

    /// 和 `retention.rs`、`tests/privacy.rs` 同一套自建暫存目錄。
    /// 不引 `tempfile` 的理由見 retention 那邊：相依樹是被盯著的資產。
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-pause-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn windows_child_missing_with_non_directory_parent_is_paused() {
        // Linux 的 child `try_exists` 會先回 Err，跑不出 Windows 的 Ok(false) 組合。
        assert!(decide(Ok(false), DirState::NotADir));
    }

    #[test]
    fn absent_data_dir_is_not_paused() {
        assert!(!decide(Ok(false), DirState::Absent));
    }

    #[test]
    fn directory_without_flag_is_not_paused() {
        assert!(!decide(Ok(false), DirState::Dir));
    }

    #[test]
    fn present_flag_and_unreadable_child_are_paused() {
        assert!(decide(Ok(true), DirState::NotADir));
        assert!(decide(Err(()), DirState::Dir));
    }

    #[test]
    fn shell_maps_real_paths_to_the_right_states() {
        let tmp = Tmp::new("shell-states");
        assert!(!is_paused(tmp.path()));

        let absent = tmp.0.join("absent");
        assert!(!is_paused(&absent));

        let file = tmp.0.join("file");
        std::fs::write(&file, "not a directory").unwrap();
        assert!(is_paused(&file));
        // Windows 對這個真實路徑的 child 查詢回 Ok(false)；Linux 無法自然產生，
        // 所以在 IO 邊界明確餵入該答案，並仍由薄殼讀取真實 data dir 狀態。
        assert_eq!(decide_for(&file, Ok(false)), PauseDecision::PathUnreadable);
    }

    #[test]
    /// **這是唯一真的走過 `is_paused` 的 Windows 那一格的測試。**
    /// Linux 上它從 `Err(NotADirectory)` 過關，證不了什麼；Windows 上它從
    /// `Ok(false)` 過關，而那格在 alpha.75 之前會回「沒暫停」——使用者按了
    /// 暫停，她繼續錄。上面的 `decide` 單元測試碰不到薄殼，別拿它們當理由刪這條。
    fn unreadable_path_is_paused_fail_closed() {
        let tmp = Tmp::new("fail-closed");
        std::fs::remove_dir_all(&tmp.0).unwrap();
        std::fs::write(&tmp.0, "not a directory").unwrap();
        assert!(is_paused(&tmp.0));
    }

    #[test]
    fn a_fresh_machine_is_recording() {
        let dir = Tmp::new("fresh");
        assert!(!is_paused(dir.path()));
    }

    #[test]
    fn inspecting_an_absent_data_dir_does_not_create_control_files() {
        let dir = Tmp::new("read-has-no-side-effects");
        let absent = dir.path().join("not-created");

        assert!(matches!(
            snapshot(&absent),
            PauseSnapshot::Recording(PauseEpoch::INITIAL)
        ));
        assert!(
            !absent.exists(),
            "a status read must not create the data dir"
        );
        assert!(!lock_path(&absent).exists());
    }

    #[test]
    fn state_and_is_paused_agree_for_every_state() {
        let dir = Tmp::new("state-agrees");
        let cases = [
            (dir.0.join("absent"), PauseState::Recording),
            (dir.0.join("valid"), PauseState::Since(1234)),
            (
                dir.0.join("broken-flag"),
                PauseState::FlagPresentButUnreadable,
            ),
            #[cfg(unix)]
            (dir.0.join("flag-uncheckable"), PauseState::FlagUncheckable),
            (dir.0.join("not-a-dir"), PauseState::PathUnreadable),
        ];
        std::fs::create_dir_all(&cases[1].0).unwrap();
        std::fs::write(flag_path(&cases[1].0), "1234").unwrap();
        std::fs::create_dir_all(&cases[2].0).unwrap();
        std::fs::write(flag_path(&cases[2].0), "half-written").unwrap();
        #[cfg(unix)]
        {
            std::fs::create_dir_all(&cases[3].0).unwrap();
            std::os::unix::fs::symlink("paused.flag", flag_path(&cases[3].0)).unwrap();
            assert!(flag_path(&cases[3].0).try_exists().is_err());
            std::fs::write(&cases[4].0, "not a directory").unwrap();
        }
        #[cfg(not(unix))]
        std::fs::write(&cases[3].0, "not a directory").unwrap();

        for (path, expected) in cases {
            let actual = state(&path);
            assert_eq!(actual, expected, "{}", path.display());
            assert_eq!(
                matches!(actual, PauseState::Recording),
                !is_paused(&path),
                "state() 和 is_paused() 對不起來：{}",
                path.display()
            );
        }
    }

    #[test]
    fn the_flag_survives_the_process_that_set_it() {
        // 「不同行程」這件事在測試裡就是「不共用任何記憶體」——只有路徑。
        let dir = Tmp::new("survives");
        set_paused(dir.path(), true, 1_700_000_000_000).unwrap();
        assert!(is_paused(dir.path()));
        assert_eq!(paused_since(dir.path()), Some(1_700_000_000_000));

        set_paused(dir.path(), false, 1_700_000_009_999).unwrap();
        assert!(!is_paused(dir.path()));
        assert_eq!(paused_since(dir.path()), None);
    }

    #[test]
    fn pausing_twice_does_not_reset_the_clock() {
        // 不然使用者每按一次暫停，「已經暫停 3 小時」就變回 0。
        let dir = Tmp::new("twice");
        set_paused(dir.path(), true, 1000).unwrap();
        set_paused(dir.path(), true, 9999).unwrap();
        assert_eq!(paused_since(dir.path()), Some(1000));
    }

    #[test]
    fn resuming_when_already_recording_is_not_an_error() {
        let dir = Tmp::new("resume-noop");
        set_paused(dir.path(), false, 1).unwrap();
        assert!(!is_paused(dir.path()));
    }

    #[test]
    fn a_flag_we_cannot_read_still_means_paused() {
        // 寫到一半斷電：檔案在，內容是空的。判定仍然必須是「暫停」。
        let dir = Tmp::new("empty");
        std::fs::write(flag_path(dir.path()), "").unwrap();
        assert!(is_paused(dir.path()));
        assert_eq!(paused_since(dir.path()), None);
    }

    #[test]
    fn garbage_in_the_flag_does_not_start_the_camera() {
        let dir = Tmp::new("garbage");
        std::fs::write(flag_path(dir.path()), "當然不是一個數字").unwrap();
        assert!(is_paused(dir.path()));
        assert_eq!(paused_since(dir.path()), None);
    }

    #[test]
    fn pausing_works_before_anyone_has_made_the_data_dir() {
        // 桌面程式可能比 `record` 先被打開。那時候 data dir 還不存在，
        // 但「我要暫停」不該因此失敗——否則第一次按就是靜默失效。
        let dir = Tmp::new("no-data-dir");
        let nested = dir.path().join("does").join("not").join("exist");
        set_paused(&nested, true, 42).unwrap();
        assert!(is_paused(&nested));
    }

    fn known_epoch(snapshot: PauseSnapshot) -> PauseEpoch {
        snapshot.epoch().expect("expected a known pause epoch")
    }

    #[test]
    fn completed_pause_interval_changes_generation_and_keeps_both_boundaries() {
        let dir = Tmp::new("completed-generation");
        let before = known_epoch(snapshot(dir.path()));
        assert_eq!(before, PauseEpoch::INITIAL);

        set_paused(dir.path(), true, 1_000).unwrap();
        let during = match snapshot(dir.path()) {
            PauseSnapshot::Paused(epoch) => epoch,
            other => panic!("expected paused, got {other:?}"),
        };
        assert_eq!(during.generation().get(), 1);
        assert_eq!(during.paused_at(), Some(1_000));
        assert_eq!(during.resumed_at(), None);

        set_paused(dir.path(), false, 1_900).unwrap();
        let after = match snapshot(dir.path()) {
            PauseSnapshot::Recording(epoch) => epoch,
            other => panic!("expected recording, got {other:?}"),
        };
        assert_eq!(after.generation(), during.generation());
        assert_ne!(after.generation(), before.generation());
        assert_eq!(after.paused_at(), Some(1_000));
        assert_eq!(after.resumed_at(), Some(1_900));
        assert!(
            state_path(dir.path()).is_file(),
            "resume must retain generation"
        );
    }

    #[test]
    fn repeated_pause_is_idempotent_and_next_real_pause_advances_once() {
        let dir = Tmp::new("generation-idempotence");
        set_paused(dir.path(), true, 10).unwrap();
        set_paused(dir.path(), true, 99).unwrap();
        let first = known_epoch(snapshot(dir.path()));
        assert_eq!(first.generation().get(), 1);
        assert_eq!(first.paused_at(), Some(10));

        set_paused(dir.path(), false, 100).unwrap();
        set_paused(dir.path(), false, 101).unwrap();
        assert_eq!(known_epoch(snapshot(dir.path())).generation().get(), 1);

        set_paused(dir.path(), true, 200).unwrap();
        let second = known_epoch(snapshot(dir.path()));
        assert_eq!(second.generation().get(), 2);
        assert_eq!(second.paused_at(), Some(200));
    }

    #[test]
    fn legacy_empty_flag_is_paused_and_resume_assigns_a_persistent_generation() {
        let dir = Tmp::new("legacy-empty-generation");
        std::fs::write(flag_path(dir.path()), "").unwrap();
        let legacy = match snapshot(dir.path()) {
            PauseSnapshot::Paused(epoch) => epoch,
            other => panic!("empty legacy flag must pause, got {other:?}"),
        };
        assert_eq!(legacy.generation().get(), 0);
        assert_eq!(legacy.paused_at(), None);

        set_paused(dir.path(), false, 700).unwrap();
        let resumed = match snapshot(dir.path()) {
            PauseSnapshot::Recording(epoch) => epoch,
            other => panic!("expected migrated recording state, got {other:?}"),
        };
        assert_eq!(resumed.generation().get(), 1);
        assert_eq!(
            resumed.paused_at(),
            None,
            "unknown legacy time stays unknown"
        );
        assert_eq!(resumed.resumed_at(), Some(700));
        assert!(!flag_path(dir.path()).exists());
    }

    #[test]
    fn crash_after_flag_publish_cannot_hide_the_pause_generation() {
        let dir = Tmp::new("crash-after-flag");
        std::fs::write(flag_path(dir.path()), "v1 1 123").unwrap();

        let interrupted = match snapshot(dir.path()) {
            PauseSnapshot::Paused(epoch) => epoch,
            other => panic!("published flag must pause, got {other:?}"),
        };
        assert_eq!(interrupted.generation().get(), 1);
        assert_eq!(interrupted.paused_at(), Some(123));

        set_paused(dir.path(), false, 456).unwrap();
        let recovered = match snapshot(dir.path()) {
            PauseSnapshot::Recording(epoch) => epoch,
            other => panic!("resume should finish interrupted pause, got {other:?}"),
        };
        assert_eq!(recovered.generation().get(), 1);
        assert_eq!(recovered.resumed_at(), Some(456));
    }

    #[test]
    fn crash_after_resume_state_keeps_flag_authoritative_and_retry_time_stable() {
        let dir = Tmp::new("crash-during-resume");
        set_paused(dir.path(), true, 100).unwrap();
        write_state_atomic(
            dir.path(),
            PersistedPauseState {
                version: STATE_VERSION,
                generation: 1,
                paused: false,
                paused_at: Some(100),
                resumed_at: Some(200),
            },
        )
        .unwrap();
        assert!(matches!(snapshot(dir.path()), PauseSnapshot::Paused(_)));

        set_paused(dir.path(), false, 999).unwrap();
        let recovered = match snapshot(dir.path()) {
            PauseSnapshot::Recording(epoch) => epoch,
            other => panic!("retry should remove the retained flag, got {other:?}"),
        };
        assert_eq!(recovered.generation().get(), 1);
        assert_eq!(recovered.paused_at(), Some(100));
        assert_eq!(
            recovered.resumed_at(),
            Some(200),
            "retry must not move the original resume boundary"
        );
    }

    #[test]
    fn corrupt_persistent_state_is_typed_indeterminate_and_fail_closed() {
        let dir = Tmp::new("corrupt-state");
        std::fs::write(state_path(dir.path()), b"{half-written").unwrap();

        assert_eq!(snapshot(dir.path()), PauseSnapshot::Indeterminate);
        assert!(is_paused(dir.path()));
        assert_eq!(state(dir.path()), PauseState::ControlStateUncheckable);
        assert!(set_paused(dir.path(), false, 12).is_err());
    }

    #[test]
    fn valid_flag_does_not_hide_a_corrupt_generation_record() {
        let dir = Tmp::new("corrupt-state-with-flag");
        std::fs::write(state_path(dir.path()), b"{half-written").unwrap();
        std::fs::write(flag_path(dir.path()), "123").unwrap();

        assert_eq!(snapshot(dir.path()), PauseSnapshot::Indeterminate);
        assert_eq!(state(dir.path()), PauseState::ControlStateUncheckable);
        assert!(is_paused(dir.path()));
        assert!(set_paused(dir.path(), false, 456).is_err());
    }

    #[test]
    fn atomic_publish_replaces_an_existing_destination() {
        // This is specifically a Windows contract test: std::fs::rename cannot replace
        // the destination there, while pause/resume rewrites pause.state every time.
        let dir = Tmp::new("atomic-replace-existing");
        let destination = dir.path().join("replace-me");
        atomic_write(dir.path(), &destination, b"first").unwrap();
        atomic_write(dir.path(), &destination, b"second").unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"second");
    }

    #[test]
    fn exhausted_generation_still_publishes_a_fail_closed_legacy_flag() {
        let dir = Tmp::new("generation-overflow");
        write_state_atomic(
            dir.path(),
            PersistedPauseState {
                version: STATE_VERSION,
                generation: u64::MAX,
                paused: false,
                paused_at: Some(1),
                resumed_at: Some(2),
            },
        )
        .unwrap();

        assert!(set_paused(dir.path(), true, 3).is_err());
        assert!(flag_path(dir.path()).is_file());
        assert!(is_paused(dir.path()));
    }

    #[test]
    fn unreadable_state_prevents_resume_from_deleting_the_flag() {
        let dir = Tmp::new("state-read-failure");
        std::fs::write(flag_path(dir.path()), "123").unwrap();
        std::fs::create_dir(state_path(dir.path())).unwrap();

        assert!(set_paused(dir.path(), false, 456).is_err());
        assert!(
            flag_path(dir.path()).is_file(),
            "resume must not delete the only fail-closed signal before state is durable"
        );
        assert!(is_paused(dir.path()));
    }

    #[test]
    fn shared_snapshot_guard_excludes_a_pause_writer_until_commit_finishes() {
        let dir = Tmp::new("shared-guard");
        let guard = snapshot_guard(dir.path());
        assert!(matches!(guard.snapshot(), PauseSnapshot::Recording(_)));

        let contender = open_lock_file(dir.path()).unwrap();
        assert!(
            matches!(
                fs4::FileExt::try_lock(&contender),
                Err(fs4::TryLockError::WouldBlock)
            ),
            "exclusive pause writer must not cross a live Recording guard"
        );
        drop(guard);
        fs4::FileExt::try_lock(&contender).expect("writer can linearize after guard drops");
    }

    #[test]
    fn simultaneous_transactional_toggles_each_take_effect_once() {
        let dir = Tmp::new("concurrent-toggle");
        let path = Arc::new(dir.0.clone());
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for ts in [100, 200] {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                toggle_paused(&path, ts).expect("transactional toggle")
            }));
        }
        barrier.wait();
        let mut results: Vec<bool> = handles
            .into_iter()
            .map(|handle| handle.join().expect("toggle thread"))
            .collect();
        results.sort_unstable();
        assert_eq!(results, vec![false, true]);

        let final_epoch = match snapshot(dir.path()) {
            PauseSnapshot::Recording(epoch) => epoch,
            other => panic!("two toggles should return to recording, got {other:?}"),
        };
        assert_eq!(final_epoch.generation().get(), 1);
        assert!(final_epoch.paused_at().is_some());
        assert!(final_epoch.resumed_at().is_some());
    }
}
