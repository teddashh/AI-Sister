//! 全停開關放在 `sister-hands`，因為它是三層共用依賴的最底層。
//!
//! `sister-core` 與 `sister-capture` 都已經向下依賴 hands，反過來卻不成立；把這條
//! 跨 capture／brain／hands 的 durable latch 放在任何更高層，都會形成反向相依或
//! 留下一層讀不到。data dir 裡的一個檔案讓不同程序看見同一個狀態，行程重開後仍保留。
//!
//! 三條規則和拔手開關相同：不確定就是停止、不會自己過期、第一次停止的時間不能
//! 被重按洗掉。Operational gate 會一起驗 latch、pending 與三顆永久鎖；內容中的時間
//! 只用來顯示，但 malformed／非一般檔案會進 `Uncertain`，不會冒充已排乾完成。

use fs4::FileExt;
use serde::Serialize;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SWITCH: &str = "master.stop";
const PENDING: &str = "master.stop.pending";
pub const ACTIVITY_LOCK: &str = "master.stop.lock";
pub const TURNSTILE_LOCK: &str = "master.stop.turnstile";
pub const OWNER_LOCK: &str = "master.stop.owner";

pub fn switch_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SWITCH)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DirState {
    Dir,
    NotADir,
    Absent,
    Unreadable,
}

/// 給人看的全停狀態。Operational gate 仍然只問「能不能開始新工作」，但畫面
/// 必須分得出正在排乾、真的排乾完成，以及根本讀不懂協定三種情況。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    #[default]
    Clear,
    Stopping,
    Stopped,
    Uncertain,
}

fn dir_state(data_dir: &Path) -> DirState {
    match std::fs::symlink_metadata(data_dir) {
        // ActivityGuard retains a path, not an opened directory handle. Refuse
        // an already-indirected data-dir entry instead of silently starting a
        // protocol through it. This is not a namespace pin: moving/replacing
        // the path (or retargeting an ancestor) while AI-Sister is running
        // requires same-user filesystem mutation and remains out of scope.
        Ok(entry) if data_dir_entry_is_indirection(&entry) => DirState::Unreadable,
        Ok(entry) if entry.is_dir() => DirState::Dir,
        Ok(_) => DirState::NotADir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => DirState::Absent,
        Err(_) => DirState::Unreadable,
    }
}

fn data_dir_entry_is_indirection(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    }
    #[cfg(not(windows))]
    false
}

fn decide(child: Result<bool, ()>, dir: DirState) -> bool {
    match child {
        Ok(true) | Err(()) => true,
        Ok(false) => match dir {
            DirState::Dir | DirState::Absent => false,
            DirState::NotADir | DirState::Unreadable => true,
        },
    }
}

fn decide_for(data_dir: &Path, child: Result<bool, ()>) -> bool {
    decide(child, dir_state(data_dir))
}

/// 開關在不在。**用 `symlink_metadata`，不要用 `try_exists`。**
///
/// `try_exists` 會**跟著 symlink 走**，所以一條指向不存在目標的 symlink
/// 會回 `Ok(false)`＝「我確定開關不在」。那是 fail-open，而且是可以從外面
/// 佈置的：先在 data dir 放一條斷掉的 `master.stop` symlink，之後那個關就再也關不上，
/// 而 `create_new` 又會因為那條路徑已存在而回 `AlreadyExists`（被當成「本來就關著」），
/// 於是命令說「已經關了」、閘門說「沒關」，兩句話同時印在同一個產品裡。
/// alpha.118 實測重現過。
///
/// `symlink_metadata` 不跟著走：目錄項存在就算存在，這才符合「不確定就是關」。
fn switch_present(path: &Path) -> Result<bool, ()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
}

fn pending_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PENDING)
}

fn stop_requested(data_dir: &Path) -> bool {
    match dir_state(data_dir) {
        DirState::Absent => false,
        DirState::NotADir | DirState::Unreadable => true,
        DirState::Dir => {
            !permanent_locks_usable(data_dir)
                || decide_for(data_dir, switch_present(&switch_path(data_dir)))
                || decide_for(data_dir, switch_present(&pending_path(data_dir)))
                || !activity_lock_is_uncontended(data_dir)
        }
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

fn opened_file_has_single_link(_file: &File, _metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        _metadata.nlink() == 1
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(HANDLE(_file.as_raw_handle()), &mut information).is_ok()
                && information.nNumberOfLinks == 1
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = _file;
        let _ = _metadata;
        true
    }
}

fn opened_file_is_single_link_regular_non_reparse(file: &File, metadata: &Metadata) -> bool {
    opened_handle_is_regular(metadata) && opened_file_has_single_link(file, metadata)
}

fn timestamp_path_state(path: &Path) -> Result<bool, ()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) if read_timestamp(path).is_ok() => Ok(true),
        Ok(_) => Err(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
}

fn permanent_locks_usable(data_dir: &Path) -> bool {
    [ACTIVITY_LOCK, TURNSTILE_LOCK, OWNER_LOCK]
        .into_iter()
        // 這三顆本來就是永久協定檔。existing empty dir 裡缺檔時，單看 metadata
        // 回 Clear 仍不夠：若目錄不可寫，真正 admission 隨後建不出鎖，畫面就會
        // 一邊說「沒有全停」、一邊拒絕所有工作。直接 open-or-create 並驗 handle，
        // 讓 presentation state 與 operational gate 問同一件事。open 成功仍不夠：
        // 某些 filesystem／handle 可能不能做 whole-file lock；那時 admission 會
        // fail closed，畫面也必須是 Uncertain，不能顯示 Clear。
        .all(|name| {
            let Ok(file) = open_lock(data_dir, name) else {
                return false;
            };
            match FileExt::try_lock_shared(&file) {
                Ok(()) => FileExt::unlock(&file).is_ok(),
                // 另一個行程正持 exclusive 本身就證明這把鎖可用。
                Err(fs4::TryLockError::WouldBlock) => true,
                Err(fs4::TryLockError::Error(_)) => false,
            }
        })
}

/// 沒有 latch／pending 時，activity 的 exclusive contention 不是一個可呈現為
/// `Clear` 的狀態。合法 engage 一定先 durable 發佈 pending，才去等 activity(X)；
/// 因此「沒有 marker、卻有人獨占 activity」只可能是孤立使用者、損壞協定或
/// 無法辨識的競態。Admission 會在同一把鎖上卡住，狀態也必須 fail closed。
fn activity_lock_is_uncontended(data_dir: &Path) -> bool {
    let Ok(activity) = open_lock(data_dir, ACTIVITY_LOCK) else {
        return false;
    };
    match FileExt::try_lock_shared(&activity) {
        Ok(()) => FileExt::unlock(&activity).is_ok(),
        Err(fs4::TryLockError::WouldBlock | fs4::TryLockError::Error(_)) => false,
    }
}

fn activity_lock_is_drained(data_dir: &Path) -> bool {
    let Ok(activity) = open_lock(data_dir, ACTIVITY_LOCK) else {
        return false;
    };
    match FileExt::try_lock(&activity) {
        Ok(()) => FileExt::unlock(&activity).is_ok(),
        Err(fs4::TryLockError::WouldBlock | fs4::TryLockError::Error(_)) => false,
    }
}

fn open_lock(data_dir: &Path, name: &str) -> io::Result<File> {
    std::fs::create_dir_all(data_dir)?;
    if dir_state(data_dir) != DirState::Dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 不是可直接使用的一般資料目錄", data_dir.display()),
        ));
    }
    let path = data_dir.join(name);
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
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0);
    }
    let file = options.open(&path)?;
    let metadata = file.metadata()?;
    if !opened_file_is_single_link_regular_non_reparse(&file, &metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} 不是可獨立使用的單連結一般、非 reparse 鎖檔",
                path.display()
            ),
        ));
    }
    Ok(file)
}

fn open_existing_lock(data_dir: &Path, name: &str) -> io::Result<File> {
    if dir_state(data_dir) != DirState::Dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 不是可直接使用的一般資料目錄", data_dir.display()),
        ));
    }
    let path = data_dir.join(name);
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0);
    }
    let file = options.open(&path)?;
    let metadata = file.metadata()?;
    if !opened_file_is_single_link_regular_non_reparse(&file, &metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} 不是可獨立使用的單連結一般、非 reparse 鎖檔",
                path.display()
            ),
        ));
    }
    Ok(file)
}

fn open_timestamp(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A FIFO/device marker must reach opened-handle validation instead of
        // blocking forever in open(2) before we can reject its file type.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            // A status poll must not make engage/release fail when either one
            // removes its marker. The permanent lock openers intentionally do
            // not share delete access.
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !opened_file_is_single_link_regular_non_reparse(&file, &metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 不是單連結的一般、非 reparse 狀態檔", path.display()),
        ));
    }
    if metadata.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 的毫秒時戳超過 64 bytes", path.display()),
        ));
    }
    Ok(file)
}

fn read_timestamp(path: &Path) -> io::Result<i64> {
    let file = open_timestamp(path)?;
    // `metadata.len()` 只是 open 後某一刻的快照；另一個行程仍可能在這兩行之間
    // 把一般檔案長大。只讓 reader 交出 65 bytes，才能讓 64-byte 上限是真的
    // I/O 上限，而不只是一次容易過期的預檢。
    let mut bytes = Vec::with_capacity(65);
    file.take(65).read_to_end(&mut bytes)?;
    if bytes.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 的毫秒時戳超過 64 bytes", path.display()),
        ));
    }
    let value = String::from_utf8(bytes).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 的毫秒時戳不是 UTF-8：{error}", path.display()),
        )
    })?;
    value.trim().parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 的毫秒時戳讀不懂：{error}", path.display()),
        )
    })
}

#[derive(Debug)]
struct ActivityLease {
    _file: File,
}

/// 一份已經在線性化閘門內獲准的活動。
///
/// guard 不是 `Copy`；clone 共享同一個活著的 OS file handle，最後一份 drop 前，全停
/// 的 exclusive drain 都不會完成。
#[derive(Clone, Debug)]
pub struct ActivityGuard {
    lease: Arc<ActivityLease>,
    data_dir: PathBuf,
}

/// 一個不可逆邊界。它同時保住 activity reader 與 turnstile reader；因此全停不能在
/// 邊界檢查之後、實際寫入／spawn／OS call 之前插進來。
#[derive(Debug)]
pub struct ActivityBoundary {
    _lease: Arc<ActivityLease>,
    _turnstile: File,
}

impl ActivityGuard {
    pub fn stop_requested(&self) -> bool {
        stop_requested(&self.data_dir)
    }

    pub fn boundary(&self) -> Option<ActivityBoundary> {
        let turnstile = open_lock(&self.data_dir, TURNSTILE_LOCK).ok()?;
        FileExt::lock_shared(&turnstile).ok()?;
        if stop_requested(&self.data_dir) {
            return None;
        }
        Some(ActivityBoundary {
            _lease: Arc::clone(&self.lease),
            _turnstile: turnstile,
        })
    }

    pub fn observed_state(&self) -> State {
        state(&self.data_dir)
    }
}

/// 取得一份活動 admission。任何目錄、lock open、handle 驗證或 lock 錯誤都 fail closed。
pub fn admit(data_dir: &Path) -> Option<ActivityGuard> {
    let turnstile = open_lock(data_dir, TURNSTILE_LOCK).ok()?;
    FileExt::lock_shared(&turnstile).ok()?;
    if stop_requested(data_dir) {
        return None;
    }
    let activity = open_lock(data_dir, ACTIVITY_LOCK).ok()?;
    // 合法 engage 在等 activity(X) 以前一定已發佈 pending，上面的 gate 會先
    // 返回。若這裡仍遇到 exclusive owner，就是孤立／損壞協定；不可握著
    // turnstile 無限等，否則 engage/release 也永遠進不來修復。
    FileExt::try_lock_shared(&activity).ok()?;
    if stop_requested(data_dir) {
        return None;
    }
    drop(turnstile);
    Some(ActivityGuard {
        lease: Arc::new(ActivityLease { _file: activity }),
        data_dir: data_dir.to_path_buf(),
    })
}

/// 全停 drain 是否已完成並發佈 durable latch。
///
/// 這是給狀態呈現用的觀察，刻意不把 `master.stop.pending` 算成完成。Operational
/// gate 必須繼續用 [`is_stopped`]：pending、讀不到或協定檔損壞時都要 fail closed。
/// desktop/core 的 BlindSpots integration 要用這個 API 決定能不能說「三層都停了」。
pub fn state(data_dir: &Path) -> State {
    match dir_state(data_dir) {
        DirState::Absent => State::Clear,
        DirState::NotADir | DirState::Unreadable => State::Uncertain,
        DirState::Dir => {
            if !permanent_locks_usable(data_dir) {
                return State::Uncertain;
            }
            let stopped = timestamp_path_state(&switch_path(data_dir));
            let pending = timestamp_path_state(&pending_path(data_dir));
            match (stopped, pending) {
                (Err(()), _) | (_, Err(())) => State::Uncertain,
                (_, Ok(true)) => match open_existing_lock(data_dir, OWNER_LOCK) {
                    Ok(owner) => match FileExt::try_lock(&owner) {
                        Ok(()) => {
                            let _ = FileExt::unlock(&owner);
                            State::Uncertain
                        }
                        Err(fs4::TryLockError::WouldBlock) => State::Stopping,
                        Err(fs4::TryLockError::Error(_)) => State::Uncertain,
                    },
                    Err(_) => State::Uncertain,
                },
                (Ok(true), Ok(false)) if activity_lock_is_drained(data_dir) => State::Stopped,
                (Ok(true), Ok(false)) => State::Uncertain,
                (Ok(false), Ok(false)) if activity_lock_is_uncontended(data_dir) => State::Clear,
                (Ok(false), Ok(false)) => State::Uncertain,
            }
        }
    }
}

pub fn is_engaged(data_dir: &Path) -> bool {
    state(data_dir) == State::Stopped
}

/// **這一行沒有任何 Linux 測試守得住，別照 Linux 的綠燈改它。**
/// 理由整段寫在 [`crate::kill_switch::is_pulled`] 上，一字不改地適用於這裡：
/// 把它寫成 `switch_path(data_dir).try_exists().unwrap_or(true)` 在 Linux 上
/// 觀察不出差別（child 查詢要穿過 data dir 本人，所以一定先回 `Err`），
/// Windows 卻把同一個情境回成 `Ok(false)`，於是那種寫法會說「我確定開關不在」。
/// 走 `dir_state` 是為了讓 data dir 本人的狀態也進得了判斷。
pub fn is_stopped(data_dir: &Path) -> bool {
    state(data_dir) != State::Clear
}

pub fn stopped_since(data_dir: &Path) -> Option<i64> {
    (state(data_dir) == State::Stopped)
        .then(|| read_timestamp(&switch_path(data_dir)).ok())
        .flatten()
}

pub fn engage(data_dir: &Path, now_ms: i64) -> std::io::Result<()> {
    engage_with_pending_observer(data_dir, now_ms, || {})
}

/// 啟動全停，並在 pending 已安全發佈、`State::Stopping` 已可被其他程序觀察之後通知呼叫端。
/// observer 只用來刷新畫面；完成通知仍以這個函式回傳為準。
pub fn engage_with_pending_observer<F>(
    data_dir: &Path,
    now_ms: i64,
    pending_observer: F,
) -> std::io::Result<()>
where
    F: FnOnce(),
{
    // Lock protocol：
    //
    // * admission：turnstile(S) -> activity(S)，取得 activity 後放 turnstile；
    // * boundary：已持 activity(S)，只短暫取得 turnstile(S)；
    // * engage：owner(X) -> turnstile(X) 發佈 pending，放 turnstile 後取得 activity(X)；
    // * release：owner(X) -> turnstile(X)，若 engage 正在排乾便等它完成後才解除。
    //
    // owner 讓 pending 有一個可驗證的活程序，也讓 release 成功回來後，舊 engage 不可能
    // 再補寫 latch。engage 等 activity 時不持有 turnstile，所以 boundary 不會 ABBA。
    let owner = open_lock(data_dir, OWNER_LOCK)?;
    FileExt::lock(&owner)?;
    let turnstile = open_lock(data_dir, TURNSTILE_LOCK)?;
    FileExt::lock(&turnstile)?;
    let pending = pending_path(data_dir);
    let requested_at = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
    {
        Ok(mut file) => {
            file.write_all(now_ms.to_string().as_bytes())?;
            now_ms
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            read_timestamp(&pending)?
        }
        Err(error) => return Err(error),
    };
    // pending 已經在 turnstile 內發佈；先放開 turnstile，讓舊 reader 的下一個
    // boundary 看見它並丟棄 RAM。若拿著 turnstile 等 activity，兩邊會互等。
    drop(turnstile);
    pending_observer();

    let activity = open_lock(data_dir, ACTIVITY_LOCK)?;
    FileExt::lock(&activity)?;
    let path = switch_path(data_dir);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file.write_all(requested_at.to_string().as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            read_timestamp(&path).map(|_| ())
        }
        Err(error) => Err(error),
    }?;
    // 寫完再問一次閘門自己看不看得到。**一個回報成功卻沒有停下來的全停，
    // 比一個大聲失敗的全停危險得多**——呼叫端會照著印「三層都停了」。
    // 上面每一條路都可能在某種檔案系統形狀下「成功」而閘門仍讀成沒停
    // （斷掉的 symlink 是實測過的一種），所以這裡不推理，直接量。
    if timestamp_path_state(&path) != Ok(true) {
        return Err(std::io::Error::other(format!(
            "寫完 {} 之後，完成 latch 仍讀不到有效毫秒時戳；全停維持 fail closed，但沒有回報完成",
            switch_path(data_dir).display()
        )));
    }
    match std::fs::remove_file(&pending) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // activity(X) 已證明舊 reader 全部離開；先驗 durable markers，再明確交還
    // activity，最後才從協定的公開 observation 重讀一次。這讓「回報成功」不只
    // 靠本函式推理，也確定其他程序此刻真的能看到 Stopped。
    if timestamp_path_state(&path) != Ok(true) || timestamp_path_state(&pending) != Ok(false) {
        return Err(io::Error::other(
            "移除 pending 後 master.stop completion markers 不一致；全停維持 fail closed 但沒有回報成功",
        ));
    }
    FileExt::unlock(&activity)?;
    drop(activity);
    if state(data_dir) != State::Stopped {
        return Err(io::Error::other(
            "釋放 activity drain lock 後仍觀察不到 Stopped；全停維持 fail closed 但沒有回報成功",
        ));
    }
    Ok(())
}

pub fn release(data_dir: &Path) -> std::io::Result<()> {
    // 先和 engage 共用 owner 排隊。若全停正在排乾，解除會等那次 engage 完成，然後才
    // 移除 latch；因此「解除成功」之後不會有較舊的 engage 又把 latch 補回去。
    let owner = open_lock(data_dir, OWNER_LOCK)?;
    FileExt::lock(&owner)?;
    let turnstile = open_lock(data_dir, TURNSTILE_LOCK)?;
    FileExt::lock(&turnstile)?;
    match std::fs::remove_file(switch_path(data_dir)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match std::fs::remove_file(pending_path(data_dir)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if state(data_dir) != State::Clear {
        return Err(io::Error::other(
            "解除旗標後全停協定仍不是 clear；沒有回報三層已恢復",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sister-master-stop-{name}-{}", std::process::id()))
    }

    #[test]
    fn engage_survives_a_fresh_read_and_never_expires() {
        let dir = temp("durable");
        let reminders = [
            switch_path(&dir),
            dir.join("paused.flag"),
            dir.join("hands.stop"),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        engage(&dir, 1_000).unwrap();
        assert!(is_stopped(&dir));
        assert_eq!(state(&dir), State::Stopped);
        assert_eq!(stopped_since(&dir), Some(1_000));
        assert!(!reminders[1].exists(), "全停不准順手寫 pause");
        assert!(!reminders[2].exists(), "全停不准順手拔手");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pressing_again_keeps_the_first_timestamp() {
        let dir = temp("first-time");
        let _ = std::fs::remove_dir_all(&dir);
        engage(&dir, 1_000).unwrap();
        engage(&dir, 9_999).unwrap();
        assert_eq!(stopped_since(&dir), Some(1_000));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_in_place_of_the_data_dir_is_stopped_fail_closed() {
        let path = temp("not-a-dir");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::write(&path, b"not a directory").unwrap();
        assert!(is_stopped(&path));
        assert_eq!(state(&path), State::Uncertain);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_empty_but_unwritable_dir_is_uncertain_not_clear() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp("readonly-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(admit(&dir).is_none());
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_data_dir_path_is_stopped_fail_closed() {
        use std::os::unix::fs::symlink;
        let path = temp("unreadable-path");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&path);
        symlink(&path, &path).unwrap();
        assert!(is_stopped(&path));
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_already_symlinked_data_dir_is_uncertain_and_never_mutates_its_target() {
        use std::os::unix::fs::symlink;

        let root = temp("data-dir-symlink");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("real-data");
        let link = root.join("data-link");
        std::fs::create_dir_all(&target).unwrap();
        symlink(&target, &link).unwrap();

        assert_eq!(state(&link), State::Uncertain);
        assert!(admit(&link).is_none());
        assert!(engage(&link, 1_000).is_err());
        assert!(release(&link).is_err());
        assert!(
            !switch_path(&target).exists() && !pending_path(&target).exists(),
            "commands must not follow the data-dir indirection and mutate its target"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_timestamp_marker_fails_closed_without_blocking_open() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::time::{Duration, Instant};

        let dir = temp("fifo-marker");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let marker = switch_path(&dir);
        let marker_bytes = CString::new(marker.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(marker_bytes.as_ptr(), 0o600) }, 0);

        let started = Instant::now();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(admit(&dir).is_none());
        assert!(engage(&dir, 1_000).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "non-regular timestamp marker blocked instead of failing closed"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn an_already_junctioned_data_dir_is_uncertain_and_never_mutates_its_target() {
        let root = temp("data-dir-junction");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("real-data");
        let junction = root.join("data-junction");
        std::fs::create_dir_all(&target).unwrap();
        let command = format!(
            "mklink /J \"{}\" \"{}\" >nul",
            junction.display(),
            target.display()
        );
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/S", "/C", &command])
            .status()
            .expect("run mklink /J");
        assert!(status.success(), "mklink /J failed with {status}");

        assert_eq!(state(&junction), State::Uncertain);
        assert!(admit(&junction).is_none());
        assert!(engage(&junction, 1_000).is_err());
        assert!(release(&junction).is_err());
        assert!(
            !switch_path(&target).exists() && !pending_path(&target).exists(),
            "commands must not follow the data-dir junction and mutate its target"
        );

        std::fs::remove_dir(&junction).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_child_state_is_stopped_fail_closed() {
        assert!(decide(Err(()), DirState::Dir));
        assert!(decide(Ok(false), DirState::Unreadable));
        assert!(decide(Ok(false), DirState::NotADir));
        assert!(!decide(Ok(false), DirState::Dir));
        assert!(!decide(Ok(false), DirState::Absent));
    }

    #[test]
    fn presentation_state_keeps_clear_stopping_stopped_and_uncertain_distinct() {
        let dir = temp("presentation-state");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(state(&dir), State::Clear);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(state(&dir), State::Clear);
        let owner = open_lock(&dir, OWNER_LOCK).unwrap();
        FileExt::lock(&owner).unwrap();
        std::fs::write(pending_path(&dir), b"1000").unwrap();
        assert_eq!(state(&dir), State::Stopping);
        std::fs::write(switch_path(&dir), b"1000").unwrap();
        assert_eq!(
            state(&dir),
            State::Stopping,
            "latch existence alone does not make a live pending operation complete"
        );
        std::fs::remove_file(pending_path(&dir)).unwrap();
        drop(owner);
        assert_eq!(state(&dir), State::Stopped);
        std::fs::remove_file(switch_path(&dir)).unwrap();
        std::fs::remove_file(dir.join(ACTIVITY_LOCK)).unwrap();
        std::fs::create_dir_all(dir.join(ACTIVITY_LOCK)).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(is_stopped(&dir), "不確定時 operational gate 必須拒絕新工作");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_exclusive_activity_lock_without_markers_is_uncertain_and_does_not_admit() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = temp("orphan-activity-owner");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let activity = open_lock(&dir, ACTIVITY_LOCK).unwrap();
        FileExt::lock(&activity).unwrap();

        assert_eq!(state(&dir), State::Uncertain);
        assert!(is_stopped(&dir));

        let (tx, rx) = mpsc::channel();
        let admit_dir = dir.clone();
        let join = std::thread::spawn(move || {
            tx.send(admit(&admit_dir).is_some()).unwrap();
        });
        assert!(
            !rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "admission crossed an unexplained exclusive activity owner"
        );

        FileExt::unlock(&activity).unwrap();
        drop(activity);
        join.join().unwrap();
        assert!(
            admit(&dir).is_some(),
            "a later, fresh admission may resume after the unexplained owner releases"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_numeric_latch_cannot_claim_stopped_while_an_admitted_activity_is_live() {
        let dir = temp("forged-completion-with-reader");
        let _ = std::fs::remove_dir_all(&dir);
        let guard = admit(&dir).expect("activity admitted before forged latch");
        std::fs::write(switch_path(&dir), b"1000").unwrap();

        assert_eq!(state(&dir), State::Uncertain);
        assert!(!is_engaged(&dir));
        assert!(is_stopped(&dir), "operational gate still fails closed");

        drop(guard);
        assert_eq!(state(&dir), State::Stopped);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 一條指向不存在目標的 symlink 佈在 `master.stop` 上，全停就再也關不上——
    /// 而且 `stop-all` 會回報成功。`try_exists` 跟著 symlink 走是成因。
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_in_place_of_the_switch_is_still_stopped() {
        use std::os::unix::fs::symlink;
        let dir = temp("dangling");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        symlink(dir.join("nowhere-at-all"), switch_path(&dir)).unwrap();
        assert!(
            is_stopped(&dir),
            "斷掉的 symlink 被讀成「我確定開關不在」＝fail-open"
        );
        assert_eq!(state(&dir), State::Uncertain);
        // 而且 engage 不可以安靜地回成功——呼叫端會照著印「三層都停了」。
        // 這條 symlink 之下沒有東西可寫，所以 engage 必須讓呼叫端知道。
        let engaged = engage(&dir, 1_000);
        assert!(
            engaged.is_err() || is_stopped(&dir),
            "engage 回報成功卻沒有停：{engaged:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_parent_symlink_is_unreadable_not_absent() {
        use std::os::unix::fs::symlink;
        let root = temp("dangling-parent");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let data_dir = root.join("data-link");
        symlink(root.join("missing-target"), &data_dir).unwrap();
        assert_eq!(dir_state(&data_dir), DirState::Unreadable);
        assert!(
            is_stopped(&data_dir),
            "斷掉的 data-dir symlink 不可讀成 absent"
        );
        assert!(admit(&data_dir).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn engage_drains_an_admitted_reader_before_returning() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = temp("reader-drain");
        let _ = std::fs::remove_dir_all(&dir);
        let guard = admit(&dir).expect("reader admitted before stop");
        let (returned_tx, returned_rx) = mpsc::channel();
        let engage_dir = dir.clone();
        let join = std::thread::spawn(move || {
            let result = engage(&engage_dir, 7);
            returned_tx.send(result).unwrap();
        });

        for _ in 0..100 {
            if pending_path(&dir).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(pending_path(&dir).exists(), "engage 沒有先發佈 pending");
        assert!(guard.stop_requested());
        assert!(
            is_stopped(&dir),
            "operational gate 必須在 pending 時 fail closed"
        );
        assert!(
            !is_engaged(&dir),
            "reader 尚未排乾，狀態呈現不可以說三層已經停完"
        );
        assert!(guard.boundary().is_none());
        assert!(returned_rx.recv_timeout(Duration::from_millis(40)).is_err());
        drop(guard);
        returned_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("engage should return after reader drops")
            .unwrap();
        join.join().unwrap();
        assert!(is_stopped(&dir));
        assert!(is_engaged(&dir), "engage 回來後才是 completed/engaged");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn observer_runs_only_after_stopping_is_live_and_before_drain_finishes() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = temp("pending-observer");
        let _ = std::fs::remove_dir_all(&dir);
        let guard = admit(&dir).expect("reader admitted before stop");
        let (observed_tx, observed_rx) = mpsc::channel();
        let (returned_tx, returned_rx) = mpsc::channel();
        let engage_dir = dir.clone();
        let join = std::thread::spawn(move || {
            let observer_dir = engage_dir.clone();
            let result = engage_with_pending_observer(&engage_dir, 8, move || {
                observed_tx.send(state(&observer_dir)).unwrap();
            });
            returned_tx.send(result).unwrap();
        });

        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            State::Stopping
        );
        assert!(returned_rx.recv_timeout(Duration::from_millis(40)).is_err());
        drop(guard);
        returned_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        join.join().unwrap();
        assert_eq!(state(&dir), State::Stopped);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn orphan_and_malformed_pending_are_uncertain_not_live_progress() {
        let dir = temp("orphan-pending");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(pending_path(&dir), b"1000").unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(is_stopped(&dir));

        std::fs::write(pending_path(&dir), b"not-a-timestamp").unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(engage(&dir, 2_000).is_err());
        assert_eq!(state(&dir), State::Uncertain);
        release(&dir).unwrap();
        assert_eq!(state(&dir), State::Clear);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn release_admission_and_engage_have_no_lock_cycle_and_linearize() {
        use std::sync::mpsc;
        use std::time::Duration;

        // 重複跑同一個 deterministic reader-first 排程，兼作 lock-order stress。
        for round in 0..32 {
            let dir = temp(&format!("release-admit-engage-{round}"));
            let _ = std::fs::remove_dir_all(&dir);
            let first_reader = admit(&dir).expect("first reader");

            let (engaged_tx, engaged_rx) = mpsc::channel();
            let engage_dir = dir.clone();
            let engage_thread = std::thread::spawn(move || {
                engaged_tx.send(engage(&engage_dir, round)).unwrap();
            });
            for _ in 0..500 {
                if pending_path(&dir).exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(pending_path(&dir).exists(), "round {round}: no pending");

            // release 必須在 owner 後面等，不能提早移除 pending 讓新 reader 鑽進來，
            // 也不能成功回來後讓舊 engage 再補寫 latch。
            let (released_tx, released_rx) = mpsc::channel();
            let release_dir = dir.clone();
            let release_thread = std::thread::spawn(move || {
                released_tx.send(release(&release_dir)).unwrap();
            });
            assert!(released_rx.recv_timeout(Duration::from_millis(20)).is_err());
            assert!(engaged_rx.recv_timeout(Duration::from_millis(20)).is_err());
            assert!(
                admit(&dir).is_none(),
                "round {round}: pending publication must reject new readers"
            );
            drop(first_reader);
            engaged_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("round {round}: engage did not drain"))
                .unwrap();
            released_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("round {round}: release did not follow engage"))
                .unwrap();

            release_thread.join().unwrap();
            engage_thread.join().unwrap();
            assert_eq!(state(&dir), State::Clear);
            std::thread::sleep(Duration::from_millis(2));
            assert_eq!(
                state(&dir),
                State::Clear,
                "round {round}: old engage relatched"
            );
            assert!(
                admit(&dir).is_some(),
                "round {round}: release admits new work"
            );
            assert!(dir.join(ACTIVITY_LOCK).exists());
            assert!(dir.join(TURNSTILE_LOCK).exists());
            assert!(dir.join(OWNER_LOCK).exists());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn release_and_admission_are_linearized() {
        let dir = temp("release-admit");
        let _ = std::fs::remove_dir_all(&dir);
        engage(&dir, 1).unwrap();
        assert!(admit(&dir).is_none());
        release(&dir).unwrap();
        let guard = admit(&dir).expect("release completed before admission");
        assert!(!guard.stop_requested());
        assert!(guard.boundary().is_some());
        drop(guard);
        assert!(dir.join(ACTIVITY_LOCK).exists());
        assert!(dir.join(TURNSTILE_LOCK).exists());
        assert!(!dir.join(PENDING).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_concurrent_second_engage_preserves_the_first_pending_timestamp() {
        let dir = temp("pending-first-time");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(pending_path(&dir), b"1000").unwrap();
        engage(&dir, 9_999).unwrap();
        assert_eq!(stopped_since(&dir), Some(1_000));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn status_polling_does_not_block_marker_removal_on_windows() {
        let dir = temp("windows-status-delete-sharing");
        let _ = std::fs::remove_dir_all(&dir);
        let mut pending_reader = None;
        engage_with_pending_observer(&dir, 1, || {
            pending_reader = Some(
                open_timestamp(&pending_path(&dir))
                    .expect("open the same pending timestamp handle used by status"),
            );
        })
        .expect("engage while pending status handle remains open");
        assert!(
            pending_reader.is_some(),
            "engage observer never held the pending status handle"
        );
        assert!(
            !pending_path(&dir).exists(),
            "engage did not remove pending"
        );

        let switch_reader = open_timestamp(&switch_path(&dir))
            .expect("open the same completion timestamp handle used by status");
        release(&dir).expect("a live status handle must share marker deletion");
        assert!(!switch_path(&dir).exists(), "release did not remove latch");
        assert_eq!(state(&dir), State::Clear);
        drop(switch_reader);
        drop(pending_reader);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stopped_since_rejects_a_symlink_even_when_its_target_is_numeric() {
        use std::os::unix::fs::symlink;

        let dir = temp("stopped-since-symlink");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let outside = dir.join("outside-number");
        std::fs::write(&outside, b"1234").unwrap();
        symlink(&outside, switch_path(&dir)).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert_eq!(stopped_since(&dir), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_timestamp_is_rejected_without_an_unbounded_read() {
        let dir = temp("oversized-timestamp");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(switch_path(&dir), vec![b'1'; 65]).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert_eq!(stopped_since(&dir), None);
        assert!(engage(&dir, 2_000).is_err());
        std::fs::write(switch_path(&dir), b"not-a-timestamp").unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(engage(&dir, 2_000).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_permanent_lock_fails_closed_and_engage_reports_error() {
        use std::os::unix::fs::symlink;

        let dir = temp("symlink-lock");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        symlink(dir.join("outside"), dir.join(TURNSTILE_LOCK)).unwrap();
        assert!(
            admit(&dir).is_none(),
            "O_NOFOLLOW lock open 必須 fail closed"
        );
        assert!(engage(&dir, 1).is_err(), "壞 lock 不可回報 engage 成功");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn non_regular_permanent_lock_fails_closed_and_is_never_replaced() {
        let dir = temp("non-regular-lock");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(ACTIVITY_LOCK)).unwrap();
        assert!(admit(&dir).is_none(), "directory 不能冒充 activity lock");
        assert!(engage(&dir, 1).is_err(), "non-regular lock 不可回報成功");
        assert!(release(&dir).is_err(), "協定仍損壞時不可回報三層已恢復");
        assert!(
            dir.join(ACTIVITY_LOCK).is_dir(),
            "協定不可 unlink/replace 壞 lock 來假裝修好"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_owner_lock_is_uncertain_and_blocks_admission() {
        let dir = temp("corrupt-owner-lock");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(OWNER_LOCK)).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(
            admit(&dir).is_none(),
            "畫面說不確定時 operational gate 也要關"
        );
        assert!(engage(&dir, 1).is_err());
        assert!(release(&dir).is_err());
        assert!(dir.join(OWNER_LOCK).is_dir());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hard_link_aliases_between_protocol_locks_fail_closed_without_entering_lock_order() {
        for (label, first, second) in [
            ("activity-owner", ACTIVITY_LOCK, OWNER_LOCK),
            ("owner-turnstile", OWNER_LOCK, TURNSTILE_LOCK),
        ] {
            let dir = temp(label);
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            drop(open_lock(&dir, first).unwrap());
            std::fs::hard_link(dir.join(first), dir.join(second)).unwrap();

            assert_eq!(state(&dir), State::Uncertain, "{label}");
            assert!(admit(&dir).is_none(), "{label}");
            assert!(engage(&dir, 1).is_err(), "{label}");
            assert!(release(&dir).is_err(), "{label}");
            assert_eq!(
                std::fs::metadata(dir.join(first)).unwrap().len(),
                std::fs::metadata(dir.join(second)).unwrap().len(),
                "fixture must still be the same linked protocol file"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_valid_latch_does_not_mask_a_corrupt_pending_entry() {
        let dir = temp("latch-corrupt-pending");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(switch_path(&dir), b"1000").unwrap();
        std::fs::create_dir_all(pending_path(&dir)).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(admit(&dir).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn release_does_not_touch_pause_or_hands() {
        let dir = temp("independent");
        let _ = std::fs::remove_dir_all(&dir);
        engage(&dir, 1).unwrap();
        std::fs::write(dir.join("paused.flag"), b"2").unwrap();
        std::fs::write(dir.join("hands.stop"), b"3").unwrap();
        release(&dir).unwrap();
        assert!(!is_stopped(&dir));
        assert!(dir.join("paused.flag").exists());
        assert!(dir.join("hands.stop").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
