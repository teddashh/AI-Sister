//! 字母人對 recorder 說的話。[`crate::heartbeat`] 是反方向。
//!
//! 「請你停下來」為什麼是一個檔案，而不是去 kill 那個行程：
//!
//! `TerminateProcess` 會讓 recorder 死在半路——`end_session` 不會被寫、心跳
//! 檔留在磁碟上、正在寫的那張圖也可能只剩半個檔案。乾淨收工必須由 recorder
//! 自己做。控制檔同時也是 desktop watchdog 看得見的 durable intent：recorder
//! 拿到請求後只把 pending 搬成 consumed，不能刪掉；否則 DB finalize 隨後失敗，
//! 非零 exit 會被誤判成 crash，剛按下的停止又被自動重啟蓋掉。

use anyhow::{Context, Result};
use fs4::{FileExt, TryLockError};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const STOP_PENDING: &str = "stop.request";
const STOP_CONSUMED: &str = "stop.consumed";
const CONSENT_REVOKE_BARRIER: &str = "consent-revoke.barrier";
/// 跨行程 control transaction 的 inode。檔案本身永久保留；存在不代表有 stop。
const STOP_LOCK: &str = "stop.lock";
const MAX_MARKER_BYTES: u64 = 64;
const BARRIER_PREFIX: &[u8] = b"v1:";
const BARRIER_RANDOM_BYTES: usize = 32;
const BARRIER_BODY_BYTES: usize = BARRIER_PREFIX.len() + BARRIER_RANDOM_BYTES * 2 + 1;

/// 為什麼要求 recorder 收工。這個型別一路帶到 session 的 [`crate::model::EndReason`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Desktop 正常退出；下一個全新的 opt-in Windows login 可在完整 start barrier
    /// 下消化它。它不能和人工 Stop 共用值，否則 automatic login 無從尊重人剛按的停。
    DesktopQuit,
    Requested,
    ConsentRevoked,
}

impl StopReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::DesktopQuit => "desktop 結束的停止要求",
            Self::Requested => "使用者要求停止",
            // Revoke barrier 會在 atomic consent save **之前**先落地。因此只能
            // 說本機停止條件已出現，不能反推 consent save 已成功 commit。
            Self::ConsentRevoked => "本機記錄同意的停止條件",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::DesktopQuit => "desktop-quit",
            Self::Requested => "requested",
            Self::ConsentRevoked => "consent-revoked",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            // alpha.106 以前的 stop.request 內容。升級不能把一個真的停止意圖
            // 變成讀不懂，所以舊字串仍算 Requested。
            "stop" | "requested" => Some(Self::Requested),
            "desktop-quit" => Some(Self::DesktopQuit),
            "consent-revoked" => Some(Self::ConsentRevoked),
            _ => None,
        }
    }
}

impl From<StopReason> for crate::model::EndReason {
    fn from(value: StopReason) -> Self {
        match value {
            StopReason::DesktopQuit => Self::DesktopQuit,
            StopReason::Requested => Self::Requested,
            StopReason::ConsentRevoked => Self::ConsentRevoked,
        }
    }
}

/// Watchdog 對 durable intent 的完整觀察。
///
/// `Uncheckable` 不是 `Absent`。Automatic retry 只能在確定是 `Absent` 時進行；
/// 下一次全新 login 的唯一窄例外是持 exclusive transaction 讀到 `DesktopQuit`，
/// 再走完 lease／heartbeat／consent barrier。把壞檔、權限錯誤或非普通檔案猜成
/// 沒有請求，會在隱私狀態不明時自行開始錄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopIntent {
    Absent,
    Pending(StopReason),
    Consumed(StopReason),
    Uncheckable,
}

/// 第一張同意書撤回 barrier 的可觀察狀態。
///
/// `Uncheckable` 絕對不等於 `Absent`：只有明確 `Absent` 才能進入自動開始，
/// 而顯式開始在 [`StartTransactionGuard::clear`] 裡也會用同一個安全讀取再檢查一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentRevokeBarrier {
    Absent,
    Present,
    Uncheckable,
}

/// 成功 regrant commit 後嘗試清理舊 barrier 的結果。
///
/// `Superseded` 表示另一個較新的 revoke 已經換上新 generation；舊 regrant
/// 不可以把它清掉。`AlreadyAbsent` 則表示這一代 barrier 已不在；無論是
/// 另一個同一代 regrant 先清掉，還是產品外的 pathname 操作，都會先驗證
/// ordinary stop marker 已存在（沒有就補一顆）才回這個狀態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentRevokeBarrierClear {
    Cleared,
    AlreadyAbsent,
    Superseded,
}

pub fn stop_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STOP_PENDING)
}

pub fn consumed_stop_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STOP_CONSUMED)
}

pub fn consent_revoke_barrier_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CONSENT_REVOKE_BARRIER)
}

/// 一次可清舊 marker 的 start-control 線性化區間。
///
/// 呼叫端必須在取得 recorder lease **之前**先取得這個 guard，持有它跨過 lease
/// acquisition 與舊 heartbeat occupancy 檢查；只有兩者都成功才呼叫 [`Self::clear`]。
/// `clear` 會消耗 guard，在同一把 exclusive `stop.lock` 裡清 marker 後才放鎖；任何
/// 提早 return 只會 drop guard，絕不清掉原本的 stop intent。
///
/// 全 repo 的既有 lock order 是 `consent.lock → stop.lock`。持有本 guard 時不可反向
/// 取得 consent lock；core control 本身也永遠不呼叫 consent。
#[derive(Debug)]
pub struct StartTransactionGuard {
    lock: ControlLock,
    data_dir: PathBuf,
}

impl StartTransactionGuard {
    /// 在已持有 exclusive transaction 時讀 marker；這份 observation 和後續 clear
    /// 之間不會插入另一個 AI-Sister writer。Login 用它只接受上一輪 typed
    /// `DesktopQuit`，絕不把人工 `Requested`／`ConsentRevoked` 猜成可以自動解除。
    pub fn intent(&self) -> Result<StopIntent> {
        stop_intent_unlocked(&self.data_dir)
    }

    pub fn clear(self) -> Result<()> {
        clear_stop_intent_locked(&self.lock, &self.data_dir)
    }
}

/// 一張只能在對應 regrant 成功 commit **之後**使用的 barrier 清理票。
///
/// 取票應放在 `consent::mutate` 的 closure 內（此時已持有 exclusive
/// `consent.lock`），而 [`Self::clear_after_commit`] 只能放在 `mutate` 回 `Ok`
/// 之後。票的 generation 會防止舊 regrant 在放掉 consent lock 後清掉較新 revoke。
/// Drop 票不會改動任何檔案。
#[derive(Debug)]
pub struct ConsentRevokeBarrierClearTicket {
    data_dir: PathBuf,
    expected: BarrierSnapshot,
}

impl ConsentRevokeBarrierClearTicket {
    pub fn clear_after_commit(self) -> Result<ConsentRevokeBarrierClear> {
        let _lock = ControlLock::acquire(&self.data_dir)?;
        let path = consent_revoke_barrier_path(&self.data_dir);
        let Some(current) = open_barrier(&path)? else {
            // 另一個同一代 regrant 可能已先清；也可能是產品外的 pathname
            // 操作把 barrier 拿掉。兩者都不能讓「必停一次」跟著消失。
            ensure_stop_intent_after_regrant(&self.data_dir)?;
            return Ok(ConsentRevokeBarrierClear::AlreadyAbsent);
        };
        if current.snapshot != self.expected {
            return Ok(ConsentRevokeBarrierClear::Superseded);
        }
        // 快速 revoke→regrant 只是重新給 permission，不是撤銷已經發生的
        // stop。先保證 barrier 下方有一份 ordinary durable intent，才能拿掉
        // barrier；否則活著的 recorder 可能在兩次 tick 之間漏掉整次撤回。
        ensure_stop_intent_after_regrant(&self.data_dir)?;
        drop(current);
        remove_barrier_matching(&path, &self.expected)?;
        Ok(ConsentRevokeBarrierClear::Cleared)
    }
}

fn ensure_stop_intent_after_regrant(data_dir: &Path) -> Result<()> {
    let pending = open_marker(&stop_path(data_dir))?;
    let consumed = open_marker(&consumed_stop_path(data_dir))?;
    if pending.is_some() || consumed.is_some() {
        // Requested/DesktopQuit/legacy ConsentRevoked 都是已經存在的事實，regrant
        // 不得改寫、升降級或清掉它。
        return Ok(());
    }
    drop(pending);
    drop(consumed);
    let _published =
        write_marker_atomic(data_dir, &stop_path(data_dir), StopReason::ConsentRevoked)?;
    Ok(())
}

/// 開始一個顯式 start transaction；取得鎖本身不會清任何 marker。
pub fn begin_explicit_start(data_dir: &Path) -> Result<StartTransactionGuard> {
    Ok(StartTransactionGuard {
        lock: ControlLock::acquire(data_dir)?,
        data_dir: data_dir.to_path_buf(),
    })
}

/// 嘗試開始一個 desktop start transaction，但絕不排在另一個 control writer 後面等。
///
/// Desktop 的同步 Start 最多只等一個有界回覆；若這裡 blocking，呼叫端可能先宣告
/// 取消，worker 卻在稍後取得鎖並清掉 marker。`None` 明確代表 control transaction
/// 正忙，且這次呼叫沒有讀、寫或清除任何 stop intent。
pub fn try_begin_start_transaction(data_dir: &Path) -> Result<Option<StartTransactionGuard>> {
    Ok(
        ControlLock::try_exclusive(data_dir)?.map(|lock| StartTransactionGuard {
            lock,
            data_dir: data_dir.to_path_buf(),
        }),
    )
}

/// 請正在跑的 recorder 因使用者要求而收工。
pub fn request_stop(data_dir: &Path) -> Result<()> {
    request(data_dir, StopReason::Requested)
}

/// Desktop 正常退出時使用；保留成和人工 Stop 不同的 durable reason。
pub fn request_desktop_quit(data_dir: &Path) -> Result<()> {
    request(data_dir, StopReason::DesktopQuit)
}

/// 第一張同意書撤回時，先立一個獨立 durable barrier。
///
/// Barrier 不和人工 `Requested`／`DesktopQuit` 共用 marker：撤回期間遇到的
/// 人工停止必須原樣留著，不能被 reason precedence 吃掉。Recorder 的
/// [`consume_stop`] 會把活著的 barrier 投影成 [`StopReason::ConsentRevoked`]，
/// 但不會消費或刪掉它。每次 revoke 都會產生新 generation，所以舊
/// regrant 無法在競爭後清掉新的撤回。
pub fn request_consent_revoke(data_dir: &Path) -> Result<()> {
    request_consent_revoke_with_after_publish(data_dir, || {})
}

fn request_consent_revoke_with_after_publish(
    data_dir: &Path,
    after_publish: impl FnOnce(),
) -> Result<()> {
    let _lock = ControlLock::acquire(data_dir)?;
    let path = consent_revoke_barrier_path(data_dir);
    let body = fresh_barrier_body()?;
    let _published = write_barrier_atomic(data_dir, &path, &body)?;
    after_publish();
    Ok(())
}

/// 在 consent writer 的舊→新 snapshot transaction 裡取得清理票。
///
/// 只有當同一個 transaction 最後的 snapshot 已允許本機錄製時才應呼叫。
/// `Ok(None)` 是可驗證的 missing；非普通檔案、symlink/reparse、壞內容或 I/O
/// 失敗都是 `Err`，不會被冒充成 missing。
pub fn prepare_consent_revoke_barrier_clear(
    data_dir: &Path,
) -> Result<Option<ConsentRevokeBarrierClearTicket>> {
    let _lock = ControlLock::acquire(data_dir)?;
    let path = consent_revoke_barrier_path(data_dir);
    Ok(
        open_barrier(&path)?.map(|barrier| ConsentRevokeBarrierClearTicket {
            data_dir: data_dir.to_path_buf(),
            expected: barrier.snapshot.clone(),
        }),
    )
}

fn request(data_dir: &Path, reason: StopReason) -> Result<()> {
    request_with_after_lock(data_dir, reason, || {})
}

fn request_with_after_lock(
    data_dir: &Path,
    reason: StopReason,
    after_lock: impl FnOnce(),
) -> Result<()> {
    let _lock = ControlLock::acquire(data_dir)?;
    after_lock();

    // ConsentRevoked > 人工 Requested > DesktopQuit。pending 與 consumed 都要看：
    // crash 可能合法留下兩份，較弱的新要求不能把較強的使用者決定降級。
    let pending = open_marker(&stop_path(data_dir))?;
    let consumed = open_marker(&consumed_stop_path(data_dir))?;
    let reason = strongest_reason(
        reason,
        pending.as_ref().map(OpenedMarker::reason),
        consumed.as_ref().map(OpenedMarker::reason),
    );
    // Windows marker handle 刻意不 share delete；atomic replace 前先結束這兩個
    // 已驗證 handle。stop.lock 仍握著，所以產品自己的 writer 不可能插隊。
    drop(pending);
    drop(consumed);

    let _published = write_marker_atomic(data_dir, &stop_path(data_dir), reason)?;
    Ok(())
}

/// 讀 durable stop intent，不把「讀不到」冒充成「不存在」。
pub fn stop_intent(data_dir: &Path) -> StopIntent {
    let Ok(Some(_lock)) = ControlLock::try_shared(data_dir) else {
        return StopIntent::Uncheckable;
    };
    stop_intent_unlocked(data_dir).unwrap_or(StopIntent::Uncheckable)
}

/// 獨立觀察 consent revoke barrier；missing 與讀不到是兩個狀態。
pub fn consent_revoke_barrier(data_dir: &Path) -> ConsentRevokeBarrier {
    let Ok(Some(_lock)) = ControlLock::try_shared(data_dir) else {
        return ConsentRevokeBarrier::Uncheckable;
    };
    match open_barrier(&consent_revoke_barrier_path(data_dir)) {
        Ok(Some(_)) => ConsentRevokeBarrier::Present,
        Ok(None) => ConsentRevokeBarrier::Absent,
        Err(_) => ConsentRevokeBarrier::Uncheckable,
    }
}

fn stop_intent_unlocked(data_dir: &Path) -> Result<StopIntent> {
    if open_barrier(&consent_revoke_barrier_path(data_dir))?.is_some() {
        return Ok(StopIntent::Pending(StopReason::ConsentRevoked));
    }
    let pending = open_marker(&stop_path(data_dir))?;
    let consumed = open_marker(&consumed_stop_path(data_dir))?;
    Ok(match (pending.as_ref(), consumed.as_ref()) {
        (Some(pending), consumed) => StopIntent::Pending(strongest_reason(
            pending.reason(),
            None,
            consumed.map(OpenedMarker::reason),
        )),
        (None, Some(consumed)) => StopIntent::Consumed(consumed.reason()),
        (None, None) => StopIntent::Absent,
    })
}

fn strongest_reason(
    requested: StopReason,
    first: Option<StopReason>,
    second: Option<StopReason>,
) -> StopReason {
    if requested == StopReason::ConsentRevoked
        || first == Some(StopReason::ConsentRevoked)
        || second == Some(StopReason::ConsentRevoked)
    {
        StopReason::ConsentRevoked
    } else if requested == StopReason::Requested
        || first == Some(StopReason::Requested)
        || second == Some(StopReason::Requested)
    {
        StopReason::Requested
    } else {
        StopReason::DesktopQuit
    }
}

/// recorder 拿走一個 pending request，並留下 consumed marker。
///
/// 回 `Result<Option<StopReason>>`，讓「沒有請求」和「控制面讀不到」不會再長成同
/// 一個 `None`。呼叫端遇到 `Err` 必須立即停止擷取；不能捏造 Requested／Revoked。
/// 已經是 Consumed 不會再停第二次，但 marker 仍留給 watchdog；真人顯式開始可
/// 清掉它，而下一次全新 opt-in login 只可清 typed `DesktopQuit`。
pub fn consume_stop(data_dir: &Path) -> Result<Option<StopReason>> {
    let _lock = ControlLock::acquire(data_dir)?;
    // Revoke barrier 是另一個 consent transaction 才能解除的 durable
    // state；recorder 只投影 reason 來收工，絕不「消費」它。底下任何
    // Requested/DesktopQuit marker 也原封不動。
    if open_barrier(&consent_revoke_barrier_path(data_dir))?.is_some() {
        return Ok(Some(StopReason::ConsentRevoked));
    }
    let pending = stop_path(data_dir);
    let consumed = consumed_stop_path(data_dir);
    let pending_marker = open_marker(&pending)?;
    let consumed_marker = open_marker(&consumed)?;
    let Some(pending_marker) = pending_marker else {
        return Ok(None);
    };
    let reason = strongest_reason(
        pending_marker.reason(),
        None,
        consumed_marker.as_ref().map(OpenedMarker::reason),
    );
    let pending_snapshot = pending_marker.snapshot.clone();
    drop(pending_marker);
    drop(consumed_marker);

    // consumed 先落地並 exact readback，pending 才能消失。crash 或後一步失敗時
    // 至少仍有一份 durable intent；回 Err 會讓 recorder 立即停止而不是再 tick。
    let _published = write_marker_atomic(data_dir, &consumed, reason)?;
    remove_marker_matching(&pending, &pending_snapshot)?;
    Ok(Some(reason))
}

/// 舊呼叫端的相容 wrapper。新 recorder 應使用 [`consume_stop`] 保留 typed reason。
pub fn take_stop(data_dir: &Path) -> Result<bool> {
    consume_stop(data_dir).map(|reason| reason.is_some())
}

/// 顯式開始一場新錄製，清掉 pending 與 consumed；任何一個清不掉都回錯。
pub fn clear_stop_intent(data_dir: &Path) -> Result<()> {
    clear_stop_intent_with_after_lock(data_dir, || {})
}

fn clear_stop_intent_with_after_lock(data_dir: &Path, after_lock: impl FnOnce()) -> Result<()> {
    let lock = ControlLock::acquire(data_dir)?;
    after_lock();
    clear_stop_intent_locked(&lock, data_dir)
}

fn clear_stop_intent_locked(_lock: &ControlLock, data_dir: &Path) -> Result<()> {
    anyhow::ensure!(
        open_barrier(&consent_revoke_barrier_path(data_dir))?.is_none(),
        "本機記錄同意的撤回 barrier 仍在；顯式開始不能清掉它"
    );
    let pending_path = stop_path(data_dir);
    let consumed_path = consumed_stop_path(data_dir);

    // 先把兩份都從 opened handle 驗完再刪第一份。若第二份是 symlink／directory，
    // 不能先把唯一可信的 pending 清掉才回錯。
    let pending = open_marker(&pending_path)?;
    let consumed = open_marker(&consumed_path)?;
    let pending = pending.map(|marker| marker.snapshot.clone());
    let consumed = consumed.map(|marker| marker.snapshot.clone());

    if let Some(snapshot) = pending.as_ref() {
        remove_marker_matching(&pending_path, snapshot)?;
    }
    if let Some(snapshot) = consumed.as_ref() {
        remove_marker_matching(&consumed_path, snapshot)?;
    }
    Ok(())
}

/// 舊 desktop 的 best-effort 相容入口。新的顯式 Start 應使用 [`clear_stop_intent`]
/// 並處理錯誤；supervised child 與 watchdog retry 絕對不能 clear。下一次全新
/// Windows login 的 typed `DesktopQuit` 例外必須持 [`StartTransactionGuard`] 並走完
/// lease／heartbeat／consent barrier，不能借這支忽略錯誤的 wrapper。
pub fn clear_stop(data_dir: &Path) {
    let _ = clear_stop_intent(data_dir);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkerSnapshot {
    reason: StopReason,
    body: Vec<u8>,
}

struct OpenedMarker {
    // Windows 這個 handle 刻意不 share write/delete；只要它活著，剛驗過的 path
    // 就不能被另一個行程換 inode 或改內容。
    _file: File,
    snapshot: MarkerSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BarrierSnapshot {
    body: Vec<u8>,
}

struct OpenedBarrier {
    // 和 stop marker 一樣，Windows 上活著的 handle 不 share write/delete，
    // 所以這個 snapshot 對應的 inode 不會在驗證中途被換掉。
    _file: File,
    snapshot: BarrierSnapshot,
}

impl OpenedMarker {
    fn reason(&self) -> StopReason {
        self.snapshot.reason
    }
}

fn open_marker(path: &Path) -> Result<Option<OpenedMarker>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode(FILE_SHARE_READ.0);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("開啟 stop marker {} 失敗", path.display()));
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("驗證 stop marker {} 失敗", path.display()))?;
    anyhow::ensure!(
        opened_handle_is_regular(&metadata),
        "stop marker 不是普通的 non-reparse 檔案：{}",
        path.display()
    );

    let mut body = Vec::new();
    (&file)
        .take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut body)
        .with_context(|| format!("讀取 stop marker {} 失敗", path.display()))?;
    anyhow::ensure!(
        body.len() as u64 <= MAX_MARKER_BYTES,
        "stop marker 太大，無法信任：{}",
        path.display()
    );
    let text = std::str::from_utf8(&body)
        .with_context(|| format!("stop marker 不是 UTF-8：{}", path.display()))?;
    let reason = StopReason::parse(text)
        .with_context(|| format!("stop marker 內容不明：{}", path.display()))?;
    Ok(Some(OpenedMarker {
        _file: file,
        snapshot: MarkerSnapshot { reason, body },
    }))
}

fn open_barrier(path: &Path) -> Result<Option<OpenedBarrier>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode(FILE_SHARE_READ.0);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("開啟 consent revoke barrier {} 失敗", path.display()));
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("驗證 consent revoke barrier {} 失敗", path.display()))?;
    anyhow::ensure!(
        opened_handle_is_regular(&metadata),
        "consent revoke barrier 不是普通的 non-reparse 檔案：{}",
        path.display()
    );

    let mut body = Vec::new();
    (&file)
        .take(BARRIER_BODY_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .with_context(|| format!("讀取 consent revoke barrier {} 失敗", path.display()))?;
    anyhow::ensure!(
        barrier_body_is_canonical(&body),
        "consent revoke barrier 內容不明：{}",
        path.display()
    );
    Ok(Some(OpenedBarrier {
        _file: file,
        snapshot: BarrierSnapshot { body },
    }))
}

fn barrier_body_is_canonical(body: &[u8]) -> bool {
    body.len() == BARRIER_BODY_BYTES
        && body.starts_with(BARRIER_PREFIX)
        && body.last() == Some(&b'\n')
        && body[BARRIER_PREFIX.len()..body.len() - 1]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn fresh_barrier_body() -> Result<Vec<u8>> {
    let mut random = [0_u8; BARRIER_RANDOM_BYTES];
    getrandom::getrandom(&mut random)
        .map_err(|error| anyhow::anyhow!("產生 consent revoke barrier generation 失敗：{error}"))?;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut body = Vec::with_capacity(BARRIER_BODY_BYTES);
    body.extend_from_slice(BARRIER_PREFIX);
    for byte in random {
        body.push(HEX[(byte >> 4) as usize]);
        body.push(HEX[(byte & 0x0f) as usize]);
    }
    body.push(b'\n');
    debug_assert!(barrier_body_is_canonical(&body));
    Ok(body)
}

fn remove_marker_matching(path: &Path, expected: &MarkerSnapshot) -> Result<()> {
    let current = open_marker(path)?
        .with_context(|| format!("stop marker 在刪除前消失：{}", path.display()))?;
    anyhow::ensure!(
        current.snapshot == *expected,
        "stop marker 在刪除前改變：{}",
        path.display()
    );
    // Unix unlink 不跟 symlink；Windows 則必須先放掉 deny-delete handle 才能刪。
    // stop.lock 仍在，產品自己的 writer 不可能落在這個 close→remove 窗裡。
    drop(current);
    std::fs::remove_file(path)
        .with_context(|| format!("刪除 stop marker {} 失敗", path.display()))?;
    anyhow::ensure!(
        open_marker(path)?.is_none(),
        "stop marker 刪除後仍存在：{}",
        path.display()
    );
    Ok(())
}

fn remove_barrier_matching(path: &Path, expected: &BarrierSnapshot) -> Result<()> {
    let current = open_barrier(path)?
        .with_context(|| format!("consent revoke barrier 在刪除前消失：{}", path.display()))?;
    anyhow::ensure!(
        current.snapshot == *expected,
        "consent revoke barrier 在刪除前改變：{}",
        path.display()
    );
    drop(current);
    std::fs::remove_file(path)
        .with_context(|| format!("刪除 consent revoke barrier {} 失敗", path.display()))?;
    anyhow::ensure!(
        open_barrier(path)?.is_none(),
        "consent revoke barrier 刪除後仍存在：{}",
        path.display()
    );
    Ok(())
}

fn write_marker_atomic(
    data_dir: &Path,
    destination: &Path,
    reason: StopReason,
) -> Result<OpenedMarker> {
    // 既有 destination 必須先由 no-follow opened handle 證明是合法 marker；不能
    // 把一個 symlink/reparse 悄悄當成可覆蓋的一般檔案並回報成功。
    drop(open_marker(destination)?);

    let body = reason.as_str().as_bytes();
    publish_control_file_atomic(data_dir, destination, ".stop-tmp", body)?;
    let published = open_marker(destination)?
        .with_context(|| format!("stop marker 發布後消失：{}", destination.display()))?;
    anyhow::ensure!(
        published.snapshot.reason == reason && published.snapshot.body == body,
        "stop marker 發布後 exact readback 不符：{}",
        destination.display()
    );
    Ok(published)
}

fn write_barrier_atomic(data_dir: &Path, destination: &Path, body: &[u8]) -> Result<OpenedBarrier> {
    anyhow::ensure!(
        barrier_body_is_canonical(body),
        "拒絕發布非 canonical consent revoke barrier"
    );
    // 已存在的 barrier 必須先經 no-follow handle 驗證。壞檔不能被當作
    // missing 後情靜地覆蓋，否則一個讀不清的隱私狀態會被冒充成成功。
    drop(open_barrier(destination)?);
    publish_control_file_atomic(data_dir, destination, ".consent-revoke-tmp", body)?;
    let published = open_barrier(destination)?.with_context(|| {
        format!(
            "consent revoke barrier 發布後消失：{}",
            destination.display()
        )
    })?;
    anyhow::ensure!(
        published.snapshot.body == body,
        "consent revoke barrier 發布後 exact readback 不符：{}",
        destination.display()
    );
    Ok(published)
}

fn publish_control_file_atomic(
    data_dir: &Path,
    destination: &Path,
    temp_prefix: &str,
    body: &[u8],
) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let mut opened = None;
    let mut temp_path = None;
    for _ in 0..128 {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = data_dir.join(format!("{temp_prefix}-{}-{serial}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // 暫存檔在寫完、關閉、rename 前都不允許其他 handle 開啟或換 inode。
            options.share_mode(0);
        }
        match options.open(&candidate) {
            Ok(file) => {
                opened = Some(file);
                temp_path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("建立 control 暫存檔於 {} 失敗", data_dir.display()));
            }
        }
    }
    let mut file = opened.context("找不到可用的 control 暫存檔名")?;
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
            "原子替換 control marker {} → {} 失敗",
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
            "原子替換 control marker {} → {} 失敗",
            source.display(),
            destination.display()
        )
    })
}

#[derive(Debug)]
struct ControlLock {
    _file: File,
}

impl ControlLock {
    fn acquire(data_dir: &Path) -> Result<Self> {
        let (file, path) = Self::open(data_dir)?;
        FileExt::lock(&file)
            .with_context(|| format!("取得 stop control lock {} 失敗", path.display()))?;
        Ok(Self { _file: file })
    }

    fn try_exclusive(data_dir: &Path) -> Result<Option<Self>> {
        let (file, path) = Self::open(data_dir)?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error)
                .with_context(|| format!("試取 stop control write lock {} 失敗", path.display())),
        }
    }

    /// Observation 不能排在 writer 後面等。worker 靠這支 poll quit/cancel；若等十
    /// 幾秒，queue 裡原本已到期的 retry 可能在 Quit 被處理前先 spawn。
    fn try_shared(data_dir: &Path) -> Result<Option<Self>> {
        let (file, path) = Self::open(data_dir)?;
        match FileExt::try_lock_shared(&file) {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error)
                .with_context(|| format!("試取 stop control read lock {} 失敗", path.display())),
        }
    }

    fn open(data_dir: &Path) -> Result<(File, PathBuf)> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("建立 {} 失敗", data_dir.display()))?;
        let path = data_dir.join(STOP_LOCK);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            // 所有 owner 都要能開同一 inode 才能排隊，但刻意不 share delete；
            // 否則 live lock path 可被換成新 inode，兩邊各自以為拿到唯一鎖。
            options
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0);
        }
        let file = options
            .open(&path)
            .with_context(|| format!("開啟 stop control lock {} 失敗", path.display()))?;
        let metadata = file
            .metadata()
            .with_context(|| format!("驗證 stop control lock {} 失敗", path.display()))?;
        anyhow::ensure!(
            opened_handle_is_regular(&metadata),
            "stop control lock 不是普通的 non-reparse 檔案：{}",
            path.display()
        );
        Ok((file, path))
    }
}

fn opened_handle_is_regular(metadata: &Metadata) -> bool {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "sister-ctl-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("mkdir");
            Self(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nobody_asked_is_distinct_from_an_uncheckable_probe() {
        let absent = Tmp::new("none");
        assert_eq!(stop_intent(&absent.0), StopIntent::Absent);
        assert!(!take_stop(&absent.0).expect("absent control is checkable"));

        let broken = Tmp::new("unknown");
        std::fs::create_dir(stop_path(&broken.0)).expect("make marker a directory");
        assert_eq!(stop_intent(&broken.0), StopIntent::Uncheckable);
        assert!(
            take_stop(&broken.0).is_err(),
            "讀不懂不能偽造 absent 或 stop reason"
        );
    }

    #[test]
    fn request_is_pending_then_consumption_leaves_a_durable_marker() {
        let t = Tmp::new("once");
        request_stop(&t.0).expect("request");
        assert_eq!(
            std::fs::read_to_string(stop_path(&t.0)).expect("canonical pending marker"),
            "requested"
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested)
        );
        assert_eq!(
            consume_stop(&t.0).expect("consume"),
            Some(StopReason::Requested)
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Consumed(StopReason::Requested)
        );
        assert_eq!(
            consume_stop(&t.0).expect("second probe"),
            None,
            "同一個請求只交給 recorder 一次"
        );
        assert!(consumed_stop_path(&t.0).is_file(), "消費後不能刪掉意圖");
        assert_eq!(
            std::fs::read_to_string(consumed_stop_path(&t.0)).expect("canonical consumed marker"),
            "requested"
        );
        assert!(
            std::fs::read_dir(&t.0)
                .expect("list control dir")
                .all(|entry| !entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".stop-tmp-")),
            "successful atomic publish must not leave a temporary marker"
        );
    }

    #[test]
    fn explicit_clear_is_the_only_operation_that_removes_consumed_intent() {
        let t = Tmp::new("clear");
        request_stop(&t.0).expect("request");
        assert_eq!(
            consume_stop(&t.0).expect("consume"),
            Some(StopReason::Requested)
        );
        clear_stop_intent(&t.0).expect("explicit clear");
        assert_eq!(stop_intent(&t.0), StopIntent::Absent);
    }

    #[test]
    fn explicit_start_guard_only_clears_when_its_consuming_commit_is_called() {
        let t = Tmp::new("explicit-start-guard");
        request_stop(&t.0).expect("old request");

        let guard = begin_explicit_start(&t.0).expect("begin explicit start");
        assert_eq!(
            guard.intent().expect("locked intent is readable"),
            StopIntent::Pending(StopReason::Requested)
        );
        assert_eq!(
            std::fs::read_to_string(stop_path(&t.0)).expect("marker stays while preflight runs"),
            "requested"
        );
        drop(guard);
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested),
            "lease/heartbeat preflight failure must preserve the old marker"
        );

        begin_explicit_start(&t.0)
            .expect("retry explicit start")
            .clear()
            .expect("commit clear");
        assert_eq!(stop_intent(&t.0), StopIntent::Absent);
    }

    #[test]
    fn observation_returns_uncheckable_instead_of_waiting_for_an_exclusive_writer() {
        let t = Tmp::new("nonblocking-observation");
        request_stop(&t.0).expect("old request");
        let guard = begin_explicit_start(&t.0).expect("hold exclusive stop.lock");

        let (observed_tx, observed_rx) = mpsc::channel();
        let observed_dir = t.0.clone();
        let observer = std::thread::spawn(move || {
            observed_tx
                .send(stop_intent(&observed_dir))
                .expect("return observation");
        });
        let observed = observed_rx.recv_timeout(Duration::from_secs(2));
        drop(guard);
        observer.join().expect("observer thread");

        assert_eq!(
            observed.expect("stop_intent must not wait for the writer lock"),
            StopIntent::Uncheckable,
            "contention is uncertainty, never an invented Absent"
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested),
            "observation contention must not mutate the marker"
        );
    }

    #[test]
    fn desktop_explicit_start_does_not_wait_or_clear_behind_another_writer() {
        let t = Tmp::new("nonblocking-explicit-start");
        request_stop(&t.0).expect("old request");
        let writer = begin_explicit_start(&t.0).expect("hold exclusive stop.lock");

        assert!(
            try_begin_start_transaction(&t.0)
                .expect("contention is a typed result")
                .is_none(),
            "desktop preflight must not wait behind a stop/control writer"
        );
        drop(writer);
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested),
            "the failed try must preserve the durable marker"
        );

        try_begin_start_transaction(&t.0)
            .expect("lock state is checkable")
            .expect("released writer makes the transaction available")
            .clear()
            .expect("committed explicit start clears the old marker");
        assert_eq!(stop_intent(&t.0), StopIntent::Absent);
    }

    #[test]
    fn consent_revoke_keeps_its_reason_through_consumption_and_db_mapping() {
        let t = Tmp::new("consent");
        request_consent_revoke(&t.0).expect("revoke");
        assert_eq!(consent_revoke_barrier(&t.0), ConsentRevokeBarrier::Present);
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked)
        );
        let reason = consume_stop(&t.0)
            .expect("control is checkable")
            .expect("consume pending revoke");
        assert_eq!(reason, StopReason::ConsentRevoked);
        assert_eq!(
            crate::model::EndReason::from(reason),
            crate::model::EndReason::ConsentRevoked
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked),
            "recorder only observes the barrier; it cannot consume or delete it"
        );
        assert!(!stop_path(&t.0).exists());
        assert!(!consumed_stop_path(&t.0).exists());
        let body = std::fs::read(consent_revoke_barrier_path(&t.0)).expect("barrier body");
        assert!(barrier_body_is_canonical(&body));
        assert!(
            std::fs::read_dir(&t.0)
                .expect("list control dir")
                .all(|entry| !entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".consent-revoke-tmp-")),
            "successful atomic publish must not leave a barrier temporary file"
        );
    }

    #[test]
    fn stop_reason_labels_only_describe_the_observed_control_intent() {
        assert_eq!(StopReason::DesktopQuit.label(), "desktop 結束的停止要求");
        assert_eq!(StopReason::Requested.label(), "使用者要求停止");
        assert_eq!(StopReason::ConsentRevoked.label(), "本機記錄同意的停止條件");
    }

    #[test]
    fn desktop_quit_is_distinct_and_cannot_downgrade_manual_stop_or_revoke() {
        let plain = Tmp::new("desktop-quit-vs-stop");
        request_desktop_quit(&plain.0).expect("desktop quit");
        assert_eq!(
            stop_intent(&plain.0),
            StopIntent::Pending(StopReason::DesktopQuit)
        );
        assert_eq!(
            crate::model::EndReason::from(StopReason::DesktopQuit),
            crate::model::EndReason::DesktopQuit
        );
        assert_eq!(
            crate::model::EndReason::describe("desktop-quit"),
            "AI-Sister 結束時收工"
        );
        request_stop(&plain.0).expect("later manual stop");
        request_desktop_quit(&plain.0).expect("quit cannot downgrade manual stop");
        assert_eq!(
            stop_intent(&plain.0),
            StopIntent::Pending(StopReason::Requested)
        );

        let revoke = Tmp::new("desktop-quit-vs-revoke");
        request_consent_revoke(&revoke.0).expect("revoke");
        request_desktop_quit(&revoke.0).expect("quit cannot downgrade revoke");
        assert_eq!(
            stop_intent(&revoke.0),
            StopIntent::Pending(StopReason::ConsentRevoked)
        );
    }

    #[test]
    fn a_later_plain_stop_survives_under_the_independent_revoke_barrier() {
        let t = Tmp::new("no-downgrade");
        request_consent_revoke(&t.0).expect("revoke");
        assert_eq!(
            consume_stop(&t.0).expect("consume"),
            Some(StopReason::ConsentRevoked)
        );
        request_stop(&t.0).expect("plain stop");
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked),
            "the active barrier is the visible reason until a regrant commits"
        );
        assert_eq!(
            std::fs::read_to_string(stop_path(&t.0)).expect("manual marker is independent"),
            "requested"
        );

        let ticket = prepare_consent_revoke_barrier_clear(&t.0)
            .expect("barrier is readable")
            .expect("barrier exists");
        assert_eq!(
            ticket.clear_after_commit().expect("clear old generation"),
            ConsentRevokeBarrierClear::Cleared
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested),
            "successful regrant clears only the barrier, never the manual stop"
        );
    }

    #[test]
    fn a_consent_revoke_masks_but_does_not_overwrite_an_earlier_plain_stop() {
        let t = Tmp::new("upgrade");
        request_stop(&t.0).expect("plain stop");
        request_consent_revoke(&t.0).expect("revoke");
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked)
        );
        assert_eq!(
            consume_stop(&t.0).expect("consume"),
            Some(StopReason::ConsentRevoked)
        );
        assert_eq!(
            std::fs::read_to_string(stop_path(&t.0)).expect("plain marker survives"),
            "requested"
        );
    }

    #[test]
    fn regrant_clear_preserves_pending_or_consumed_human_and_quit_markers() {
        for (name, request, expected) in [
            (
                "requested-pending",
                request_stop as fn(&Path) -> Result<()>,
                StopIntent::Pending(StopReason::Requested),
            ),
            (
                "quit-pending",
                request_desktop_quit as fn(&Path) -> Result<()>,
                StopIntent::Pending(StopReason::DesktopQuit),
            ),
        ] {
            let t = Tmp::new(name);
            request(&t.0).expect("ordinary stop intent");
            let before = std::fs::read(stop_path(&t.0)).expect("ordinary marker bytes");
            request_consent_revoke(&t.0).expect("revoke barrier");
            let ticket = prepare_consent_revoke_barrier_clear(&t.0)
                .expect("capture")
                .expect("present");
            assert_eq!(
                ticket.clear_after_commit().expect("clear after regrant"),
                ConsentRevokeBarrierClear::Cleared
            );
            assert_eq!(stop_intent(&t.0), expected);
            assert_eq!(
                std::fs::read(stop_path(&t.0)).expect("ordinary marker survives"),
                before,
                "regrant must not rewrite a human or desktop stop marker"
            );
        }

        let consumed = Tmp::new("requested-consumed");
        request_stop(&consumed.0).expect("manual stop");
        assert_eq!(
            consume_stop(&consumed.0).expect("consume manual stop"),
            Some(StopReason::Requested)
        );
        let before =
            std::fs::read(consumed_stop_path(&consumed.0)).expect("consumed manual marker bytes");
        request_consent_revoke(&consumed.0).expect("revoke barrier");
        let ticket = prepare_consent_revoke_barrier_clear(&consumed.0)
            .expect("capture")
            .expect("present");
        assert_eq!(
            ticket.clear_after_commit().expect("clear after regrant"),
            ConsentRevokeBarrierClear::Cleared
        );
        assert_eq!(
            stop_intent(&consumed.0),
            StopIntent::Consumed(StopReason::Requested)
        );
        assert_eq!(
            std::fs::read(consumed_stop_path(&consumed.0))
                .expect("consumed manual marker survives"),
            before
        );
    }

    #[test]
    fn regrant_without_an_underlying_marker_leaves_one_stop_to_deliver() {
        let t = Tmp::new("regrant-stop-handoff");
        request_consent_revoke(&t.0).expect("pre-save revoke barrier");
        let ticket = prepare_consent_revoke_barrier_clear(&t.0)
            .expect("capture")
            .expect("barrier present");

        assert_eq!(
            ticket.clear_after_commit().expect("successful regrant"),
            ConsentRevokeBarrierClear::Cleared
        );
        assert_eq!(consent_revoke_barrier(&t.0), ConsentRevokeBarrier::Absent);
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked),
            "regrant is permission, not cancellation of the stop already requested"
        );
        assert_eq!(
            std::fs::read_to_string(stop_path(&t.0)).expect("handoff stop marker"),
            "consent-revoked"
        );
    }

    #[test]
    fn an_already_absent_captured_barrier_still_leaves_a_stop_marker() {
        let t = Tmp::new("regrant-barrier-vanished");
        request_consent_revoke(&t.0).expect("revoke barrier");
        let ticket = prepare_consent_revoke_barrier_clear(&t.0)
            .expect("capture")
            .expect("barrier present");
        std::fs::remove_file(consent_revoke_barrier_path(&t.0))
            .expect("simulate external pathname removal");
        assert_eq!(stop_intent(&t.0), StopIntent::Absent);

        assert_eq!(
            ticket
                .clear_after_commit()
                .expect("repair missing handoff marker"),
            ConsentRevokeBarrierClear::AlreadyAbsent
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked)
        );
    }

    #[test]
    fn a_stale_regrant_ticket_cannot_clear_a_newer_revoke_generation() {
        let t = Tmp::new("regrant-generation");
        request_consent_revoke(&t.0).expect("first revoke");
        let stale = prepare_consent_revoke_barrier_clear(&t.0)
            .expect("capture first")
            .expect("first barrier");
        let first = std::fs::read(consent_revoke_barrier_path(&t.0)).expect("first generation");

        request_consent_revoke(&t.0).expect("newer revoke");
        let newer = std::fs::read(consent_revoke_barrier_path(&t.0)).expect("new generation");
        assert_ne!(first, newer, "every revoke needs a fresh generation");
        assert_eq!(
            stale.clear_after_commit().expect("compare generation"),
            ConsentRevokeBarrierClear::Superseded
        );
        assert_eq!(
            std::fs::read(consent_revoke_barrier_path(&t.0)).expect("new barrier survives"),
            newer
        );
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::ConsentRevoked)
        );
    }

    #[test]
    fn missing_and_unreadable_revoke_barriers_are_distinct_and_fail_closed() {
        let missing = Tmp::new("barrier-missing");
        assert_eq!(
            consent_revoke_barrier(&missing.0),
            ConsentRevokeBarrier::Absent
        );
        assert!(
            prepare_consent_revoke_barrier_clear(&missing.0)
                .expect("missing is checkable")
                .is_none()
        );

        let broken = Tmp::new("barrier-directory");
        std::fs::create_dir(consent_revoke_barrier_path(&broken.0))
            .expect("directory-shaped barrier");
        assert_eq!(
            consent_revoke_barrier(&broken.0),
            ConsentRevokeBarrier::Uncheckable
        );
        assert_eq!(stop_intent(&broken.0), StopIntent::Uncheckable);
        assert!(prepare_consent_revoke_barrier_clear(&broken.0).is_err());
        assert!(request_consent_revoke(&broken.0).is_err());
        assert!(consume_stop(&broken.0).is_err());
        assert!(clear_stop_intent(&broken.0).is_err());
        assert!(consent_revoke_barrier_path(&broken.0).is_dir());
    }

    #[test]
    fn a_pre_save_barrier_survives_failure_and_blocks_a_waiting_explicit_clear() {
        let t = Tmp::new("revoke-save-failure");
        request_stop(&t.0).expect("older manual marker");
        let (published_tx, published_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let revoke_dir = t.0.clone();
        let revoke = std::thread::spawn(move || {
            request_consent_revoke_with_after_publish(&revoke_dir, || {
                published_tx.send(()).expect("announce durable barrier");
                release_rx
                    .recv()
                    .expect("simulate consent save failing after this point");
            })
        });
        published_rx.recv().expect("barrier published before save");

        let (start_tx, start_rx) = mpsc::channel();
        let start_dir = t.0.clone();
        let start = std::thread::spawn(move || {
            let result = begin_explicit_start(&start_dir).and_then(StartTransactionGuard::clear);
            start_tx.send(result).expect("return explicit clear");
        });
        assert!(
            start_rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "explicit start must wait while revoke still owns the transaction"
        );

        // No consent commit follows: this is the save-before-replace failure branch.
        release_tx.send(()).expect("release failed writer");
        revoke
            .join()
            .expect("join revoke")
            .expect("pre-barrier write");
        let error = start_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("explicit resumes")
            .expect_err("durable barrier refuses its clear");
        start.join().expect("join explicit");
        assert!(format!("{error:#}").contains("barrier"), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(stop_path(&t.0)).expect("manual marker was not cleared"),
            "requested"
        );
        assert_eq!(consent_revoke_barrier(&t.0), ConsentRevokeBarrier::Present);
    }

    #[test]
    fn an_old_alpha_stop_file_is_still_a_real_request() {
        let t = Tmp::new("legacy");
        std::fs::write(stop_path(&t.0), "stop").expect("legacy marker");
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested)
        );
        assert_eq!(
            consume_stop(&t.0).expect("consume legacy"),
            Some(StopReason::Requested)
        );
    }

    #[test]
    fn clear_reports_a_marker_it_cannot_remove() {
        let t = Tmp::new("clear-error");
        std::fs::create_dir(consumed_stop_path(&t.0)).expect("directory marker");
        let error = clear_stop_intent(&t.0).expect_err("directory is not a removable marker file");
        assert!(
            format!("{error:#}").contains("stop.consumed"),
            "error must identify the blocked marker: {error:#}"
        );
        assert_eq!(stop_intent(&t.0), StopIntent::Uncheckable);
    }

    #[test]
    fn stopping_does_not_touch_the_pause_flag() {
        let t = Tmp::new("orthogonal");
        crate::pause::set_paused(&t.0, true, 1_000).expect("pause");
        request_stop(&t.0).expect("request");
        assert_eq!(
            consume_stop(&t.0).expect("consume"),
            Some(StopReason::Requested)
        );
        assert!(crate::pause::is_paused(&t.0), "暫停旗標要原封不動");
    }

    #[test]
    fn a_non_regular_marker_makes_every_mutation_fail_closed() {
        let t = Tmp::new("marker-directory");
        std::fs::create_dir(stop_path(&t.0)).expect("directory at pending marker");

        assert_eq!(stop_intent(&t.0), StopIntent::Uncheckable);
        assert!(
            request_stop(&t.0).is_err(),
            "request must not report success"
        );
        assert!(consume_stop(&t.0).is_err(), "consume must not mean absent");
        assert!(
            clear_stop_intent(&t.0).is_err(),
            "clear must not report success"
        );
        assert!(
            stop_path(&t.0).is_dir(),
            "none of the operations touched it"
        );
    }

    #[test]
    fn a_non_regular_lock_makes_every_operation_fail_closed() {
        let t = Tmp::new("lock-directory");
        let lock = t.0.join(STOP_LOCK);
        std::fs::create_dir(&lock).expect("directory at stop.lock");

        assert_eq!(stop_intent(&t.0), StopIntent::Uncheckable);
        assert!(request_stop(&t.0).is_err());
        assert!(consume_stop(&t.0).is_err());
        assert!(clear_stop_intent(&t.0).is_err());
        assert!(lock.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_markers_are_never_followed_by_request_consume_or_clear() {
        use std::os::unix::fs::symlink;

        let pending_case = Tmp::new("pending-symlink");
        let pending_target = pending_case.0.join("pending-target");
        std::fs::write(&pending_target, "do not touch").expect("seed target");
        symlink(&pending_target, stop_path(&pending_case.0)).expect("pending symlink");
        assert_eq!(stop_intent(&pending_case.0), StopIntent::Uncheckable);
        assert!(request_stop(&pending_case.0).is_err());
        assert!(consume_stop(&pending_case.0).is_err());
        assert!(clear_stop_intent(&pending_case.0).is_err());
        assert_eq!(
            std::fs::read_to_string(&pending_target).expect("read target"),
            "do not touch"
        );

        let consumed_case = Tmp::new("consumed-symlink");
        let consumed_target = consumed_case.0.join("consumed-target");
        std::fs::write(&consumed_target, "do not touch").expect("seed target");
        std::fs::write(stop_path(&consumed_case.0), "requested").expect("pending marker");
        symlink(&consumed_target, consumed_stop_path(&consumed_case.0)).expect("consumed symlink");
        assert!(
            consume_stop(&consumed_case.0).is_err(),
            "Windows fallback used to follow this consumed path"
        );
        assert!(clear_stop_intent(&consumed_case.0).is_err());
        assert_eq!(
            std::fs::read_to_string(&consumed_target).expect("read target"),
            "do not touch"
        );
        assert_eq!(
            std::fs::read_to_string(stop_path(&consumed_case.0)).expect("pending survives"),
            "requested",
            "failed consume/clear must retain the trustworthy pending intent"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_revoke_barrier_is_uncheckable_and_never_followed() {
        use std::os::unix::fs::symlink;

        let t = Tmp::new("barrier-symlink");
        let target = t.0.join("barrier-target");
        std::fs::write(&target, "do not touch").expect("seed target");
        symlink(&target, consent_revoke_barrier_path(&t.0)).expect("barrier symlink");

        assert_eq!(
            consent_revoke_barrier(&t.0),
            ConsentRevokeBarrier::Uncheckable
        );
        assert_eq!(stop_intent(&t.0), StopIntent::Uncheckable);
        assert!(request_consent_revoke(&t.0).is_err());
        assert!(prepare_consent_revoke_barrier_clear(&t.0).is_err());
        assert!(consume_stop(&t.0).is_err());
        assert!(clear_stop_intent(&t.0).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "do not touch"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_control_lock_is_rejected_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let t = Tmp::new("lock-symlink");
        let target = t.0.join("lock-target");
        std::fs::write(&target, "do not touch").expect("seed target");
        symlink(&target, t.0.join(STOP_LOCK)).expect("lock symlink");

        assert_eq!(stop_intent(&t.0), StopIntent::Uncheckable);
        assert!(request_stop(&t.0).is_err());
        assert!(consume_stop(&t.0).is_err());
        assert!(clear_stop_intent(&t.0).is_err());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "do not touch"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_live_windows_control_lock_denies_delete_but_drop_releases_the_path() {
        let t = Tmp::new("windows-lock-delete-sharing");
        let guard = begin_explicit_start(&t.0).expect("open live stop.lock");
        let lock = t.0.join(STOP_LOCK);

        let error = std::fs::remove_file(&lock)
            .expect_err("live handle must omit FILE_SHARE_DELETE on Windows");
        assert!(
            lock.is_file(),
            "failed delete must leave the live lock path: {error}"
        );

        drop(guard);
        std::fs::remove_file(&lock).expect("handle drop releases delete sharing");
        assert!(!lock.exists());
    }

    #[test]
    fn a_request_overlapping_clear_is_serialized_after_clear_and_survives() {
        let t = Tmp::new("clear-request-race");
        request_stop(&t.0).expect("old request");

        let (clear_locked_tx, clear_locked_rx) = mpsc::channel();
        let (release_clear_tx, release_clear_rx) = mpsc::channel();
        let clear_dir = t.0.clone();
        let clear_thread = std::thread::spawn(move || {
            clear_stop_intent_with_after_lock(&clear_dir, || {
                clear_locked_tx.send(()).expect("announce clear lock");
                release_clear_rx.recv().expect("release clear");
            })
        });
        clear_locked_rx.recv().expect("clear owns stop.lock");

        let (request_started_tx, request_started_rx) = mpsc::channel();
        let (request_locked_tx, request_locked_rx) = mpsc::channel();
        let request_dir = t.0.clone();
        let request_thread = std::thread::spawn(move || {
            request_started_tx.send(()).expect("announce request call");
            request_with_after_lock(&request_dir, StopReason::Requested, || {
                request_locked_tx.send(()).expect("announce request lock");
            })
        });
        request_started_rx.recv().expect("request thread started");
        assert!(
            request_locked_rx
                .recv_timeout(Duration::from_millis(500))
                .is_err(),
            "request must wait while clear owns the exclusive lock"
        );

        release_clear_tx.send(()).expect("let clear commit");
        clear_thread.join().expect("clear thread").expect("clear");
        request_locked_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("request acquires after clear");
        request_thread
            .join()
            .expect("request thread")
            .expect("request");
        assert_eq!(
            stop_intent(&t.0),
            StopIntent::Pending(StopReason::Requested),
            "the later request must not be erased by the overlapping clear"
        );
        assert!(t.0.join(STOP_LOCK).is_file(), "stop.lock is permanent");
    }
}
