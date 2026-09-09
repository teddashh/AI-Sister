//! 全停開關放在 `sister-hands`，因為它是三層共用依賴的最底層。
//!
//! `sister-core` 與 `sister-capture` 都已經向下依賴 hands，反過來卻不成立；把這條
//! 跨 capture／brain／hands 的 durable latch 放在任何更高層，都會形成反向相依或
//! 留下一層讀不到。data dir 裡的一個檔案讓不同程序看見同一個狀態，行程重開後仍保留。
//!
//! 三條規則和拔手開關相同：不確定就是停止、不會自己過期、第一次停止的時間不能
//! 被重按洗掉。判定只看檔案在不在；內容只是顯示用，壞掉仍然算停止。

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
        Ok(entry) if entry.file_type().is_symlink() => match std::fs::metadata(data_dir) {
            Ok(target) if target.is_dir() => DirState::Dir,
            Ok(_) => DirState::NotADir,
            Err(_) => DirState::Unreadable,
        },
        Ok(entry) if entry.is_dir() => DirState::Dir,
        Ok(_) => DirState::NotADir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => DirState::Absent,
        Err(_) => DirState::Unreadable,
    }
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
    decide_for(data_dir, switch_present(&switch_path(data_dir)))
        || decide_for(data_dir, switch_present(&pending_path(data_dir)))
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

fn path_is_regular_non_reparse(path: &Path) -> Result<bool, ()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ())?;
    Ok(opened_handle_is_regular(&metadata))
}

fn protocol_path_state(path: &Path) -> Result<bool, ()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => match path_is_regular_non_reparse(path) {
            Ok(true) => Ok(true),
            Ok(false) | Err(()) => Err(()),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
}

fn permanent_locks_readable(data_dir: &Path) -> bool {
    [ACTIVITY_LOCK, TURNSTILE_LOCK].into_iter().all(|name| {
        match protocol_path_state(&data_dir.join(name)) {
            Ok(true) | Ok(false) => true,
            Err(()) => false,
        }
    })
}

fn open_lock(data_dir: &Path, name: &str) -> io::Result<File> {
    std::fs::create_dir_all(data_dir)?;
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
    if !opened_handle_is_regular(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 不是一般、非 reparse 的鎖檔", path.display()),
        ));
    }
    Ok(file)
}

fn read_timestamp(path: &Path) -> io::Result<i64> {
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
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0);
    }
    let mut file = options.open(path)?;
    if !opened_handle_is_regular(&file.metadata()?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 不是一般、非 reparse 的狀態檔", path.display()),
        ));
    }
    let mut value = String::new();
    file.read_to_string(&mut value)?;
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
}

/// 取得一份活動 admission。任何目錄、lock open、handle 驗證或 lock 錯誤都 fail closed。
pub fn admit(data_dir: &Path) -> Option<ActivityGuard> {
    let turnstile = open_lock(data_dir, TURNSTILE_LOCK).ok()?;
    FileExt::lock_shared(&turnstile).ok()?;
    if stop_requested(data_dir) {
        return None;
    }
    let activity = open_lock(data_dir, ACTIVITY_LOCK).ok()?;
    FileExt::lock_shared(&activity).ok()?;
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
            if !permanent_locks_readable(data_dir) {
                return State::Uncertain;
            }
            let stopped = protocol_path_state(&switch_path(data_dir));
            let pending = protocol_path_state(&pending_path(data_dir));
            match (stopped, pending) {
                (Ok(true), _) => State::Stopped,
                (Ok(false), Ok(true)) => State::Stopping,
                (Ok(false), Ok(false)) => State::Clear,
                _ => State::Uncertain,
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
    std::fs::read_to_string(switch_path(data_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn engage(data_dir: &Path, now_ms: i64) -> std::io::Result<()> {
    // Lock protocol（刻意不讓任何 mutation 同時持有兩把 lock）：
    //
    // * admission：turnstile(S) -> activity(S)，取得 activity 後放 turnstile；
    // * boundary：已持 activity(S)，只短暫取得 turnstile(S)；
    // * engage：turnstile(X) 發佈 pending，放掉後才取得 activity(X) 排乾；
    // * release：只取得 turnstile(X)，不需要排乾已准入的 activity 才能恢復。
    //
    // 因此沒有 activity -> turnstile 的 exclusive mutation 邊。Admission/release 和
    // stop-request publication 在 turnstile 上串行；engage 的 completed point 則是
    // activity(X) 內最後一次 latch 觀察。與 release 重疊時，若 release 在那次
    // 觀察後才移除 latch，就排成 engage 先、release 後；否則 engage 回錯。
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

    let activity = open_lock(data_dir, ACTIVITY_LOCK)?;
    FileExt::lock(&activity)?;
    let path = switch_path(data_dir);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => file.write_all(requested_at.to_string().as_bytes()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }?;
    // 寫完再問一次閘門自己看不看得到。**一個回報成功卻沒有停下來的全停，
    // 比一個大聲失敗的全停危險得多**——呼叫端會照著印「三層都停了」。
    // 上面每一條路都可能在某種檔案系統形狀下「成功」而閘門仍讀成沒停
    // （斷掉的 symlink 是實測過的一種），所以這裡不推理，直接量。
    if !decide_for(data_dir, switch_present(&path)) {
        return Err(std::io::Error::other(format!(
            "寫完 {} 之後，判斷閘門仍然讀成「沒有全停」；沒有停下任何一層",
            switch_path(data_dir).display()
        )));
    }
    match std::fs::remove_file(&pending) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if !is_engaged(data_dir) {
        return Err(io::Error::other(
            "移除 pending 後 master.stop latch 不可見；全停維持 fail closed 但沒有回報成功",
        ));
    }
    Ok(())
}

pub fn release(data_dir: &Path) -> std::io::Result<()> {
    // 恢復不需要等舊 activity 排乾；它只要和 admission 及 engage 的 pending publication
    // 在 turnstile 上線性化。若先拿 activity(X) 再等 turnstile(X)，會和
    // admit 的 turnstile(S) -> activity(S) 形成 ABBA deadlock。
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
    fn an_unreadable_data_dir_path_is_stopped_fail_closed() {
        use std::os::unix::fs::symlink;
        let path = temp("unreadable-path");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&path);
        symlink(&path, &path).unwrap();
        assert!(is_stopped(&path));
        std::fs::remove_file(path).unwrap();
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
        std::fs::write(pending_path(&dir), b"1000").unwrap();
        assert_eq!(state(&dir), State::Stopping);
        std::fs::write(switch_path(&dir), b"1000").unwrap();
        assert_eq!(state(&dir), State::Stopped);
        std::fs::remove_file(switch_path(&dir)).unwrap();
        std::fs::remove_file(pending_path(&dir)).unwrap();
        std::fs::create_dir_all(dir.join(ACTIVITY_LOCK)).unwrap();
        assert_eq!(state(&dir), State::Uncertain);
        assert!(is_stopped(&dir), "不確定時 operational gate 必須拒絕新工作");
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

            // 這是舊 release 的確定性死鎖形狀：engage 等 first_reader，release 若先拿
            // activity(X) 就也會等它，之後再也到不了 turnstile。新協定只取 turnstile。
            let (released_tx, released_rx) = mpsc::channel();
            let release_dir = dir.clone();
            let release_thread = std::thread::spawn(move || {
                released_tx.send(release(&release_dir)).unwrap();
            });
            released_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("round {round}: release deadlocked"))
                .unwrap();
            assert!(!is_stopped(&dir), "round {round}: release linearizes first");

            // first_reader 仍在，所以 engage 尚不可能完成。新 reader 另開 thread：
            // Windows 若對已排隊的 exclusive writer 公平，它可以合法地排在 engage
            // 後面；測試本身不能一邊握 first_reader、一邊同步等第二個 reader。
            assert!(engaged_rx.recv_timeout(Duration::from_millis(20)).is_err());
            let (admitted_tx, admitted_rx) = mpsc::channel();
            let admit_dir = dir.clone();
            let admit_thread = std::thread::spawn(move || {
                let admitted = admit(&admit_dir).is_some();
                admitted_tx.send(admitted).unwrap();
            });
            drop(first_reader);
            engaged_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("round {round}: engage did not drain"))
                .unwrap();
            let _linearizable_result = admitted_rx
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or_else(|_| panic!("round {round}: admission did not finish"));

            release_thread.join().unwrap();
            engage_thread.join().unwrap();
            admit_thread.join().unwrap();
            assert!(is_engaged(&dir), "round {round}: engage linearizes last");
            assert!(dir.join(ACTIVITY_LOCK).exists());
            assert!(dir.join(TURNSTILE_LOCK).exists());
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
