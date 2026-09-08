//! 跨行程的 recorder 單一 owner lease。
//!
//! `recording.lock` 是一個可以永久留在 data dir 的 lock 檔案；它存在只代表
//! 「這個資料目錄用過 recorder lease」，不代表現在有人錄製。唯一能回答 occupancy
//! 的是對同一檔案做 nonblocking whole-file exclusive lock 的結果。

use fs4::{FileExt, TryLockError};
use std::fmt;
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

pub const LOCK_FILE: &str = "recording.lock";

/// 取得 lease 的哪一個 I/O 階段失敗。
///
/// 這不是給呼叫端猜政策用的字串；[`AcquireError::Unknown`] 在每個階段都維持同一個
/// fail-closed 類型，operation 只讓診斷指出真正失敗的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoOperation {
    CreateDataDirectory,
    OpenLockFile,
    InspectOpenedFile,
    LockFile,
}

impl fmt::Display for IoOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CreateDataDirectory => "建立資料目錄",
            Self::OpenLockFile => "開啟 lease 檔案",
            Self::InspectOpenedFile => "驗證已開啟的 lease handle",
            Self::LockFile => "取得作業系統檔案鎖",
        })
    }
}

/// Nonblocking lease acquisition 的完整結果。
///
/// `Occupied` 是 kernel 明確回答 lock contention；任何建立／開檔／locking I/O 錯誤
/// 都是 `Unknown`。呼叫端不可把 `Unknown` 當成 vacant，否則讀不到控制狀態時反而
/// 可能開出第二個 recorder。
#[derive(Debug)]
pub enum AcquireError {
    Occupied {
        path: PathBuf,
    },
    Unknown {
        path: PathBuf,
        operation: IoOperation,
        source: io::Error,
    },
}

impl fmt::Display for AcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Occupied { path } => write!(
                f,
                "recorder lease 已被占用（{}）；沒有取得第二份 lease",
                path.display()
            ),
            Self::Unknown {
                path,
                operation,
                source,
            } => write!(
                f,
                "recorder lease 狀態不明：{operation}失敗（{}）：{source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AcquireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Occupied { .. } => None,
            Self::Unknown { source, .. } => Some(source),
        }
    }
}

/// 活著就代表這個 process 仍持有 recorder lease。
///
/// 不實作 `Clone`，避免同一個 ownership 意圖散成多份不清楚誰負責保存的 handle。
/// Drop 只需關閉底層 [`File`]；正常 drop 與行程被終止都由 OS 自動釋放 lock。
#[derive(Debug)]
pub struct RecorderLease {
    _file: File,
    path: PathBuf,
}

impl RecorderLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn lock_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOCK_FILE)
}

/// 嘗試取得 data dir 的唯一 recorder lease；永遠不等待另一個 owner。
pub fn try_acquire(data_dir: &Path) -> Result<RecorderLease, AcquireError> {
    let path = lock_path(data_dir);
    std::fs::create_dir_all(data_dir).map_err(|source| AcquireError::Unknown {
        path: path.clone(),
        operation: IoOperation::CreateDataDirectory,
        source,
    })?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // `symlink_metadata` followed by `open` leaves a swap window. O_NOFOLLOW makes the
        // no-symlink decision part of the same kernel operation that creates this handle.
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        // Open the reparse point itself rather than following it; handle metadata below rejects
        // every reparse point, including kinds std does not classify as a file symlink.
        // Deliberately omit FILE_SHARE_DELETE while the lease is live. Windows can therefore
        // keep the pathname attached to this exact inode; otherwise another process could
        // unlink the locked name, create a new file there, and acquire a second independent lock.
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0);
    }
    let file = options
        .open(&path)
        .map_err(|source| AcquireError::Unknown {
            path: path.clone(),
            operation: IoOperation::OpenLockFile,
            source,
        })?;
    let metadata = file.metadata().map_err(|source| AcquireError::Unknown {
        path: path.clone(),
        operation: IoOperation::InspectOpenedFile,
        source,
    })?;
    if !opened_handle_is_regular(&metadata) {
        return Err(AcquireError::Unknown {
            path,
            operation: IoOperation::InspectOpenedFile,
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "opened recording.lock handle is not a regular non-reparse file",
            ),
        });
    }

    match FileExt::try_lock(&file) {
        Ok(()) => Ok(RecorderLease { _file: file, path }),
        Err(TryLockError::WouldBlock) => Err(AcquireError::Occupied { path }),
        Err(TryLockError::Error(source)) => Err(AcquireError::Unknown {
            path,
            operation: IoOperation::LockFile,
            source,
        }),
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
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    const CHILD_DATA_DIR: &str = "AI_SISTER_TEST_RECORDER_LEASE_DIR";
    const CHILD_READY: &str = "AI_SISTER_TEST_RECORDER_LEASE_READY";

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "sister-recorder-lease-{}-{name}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create lease test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    #[test]
    fn an_unlocked_persistent_file_is_vacant_and_drop_releases_the_lease() {
        let dir = Tmp::new("persistent-file");
        let path = lock_path(dir.path());
        std::fs::write(&path, b"persistent marker contents are not occupancy")
            .expect("seed persistent lock file");

        let first = try_acquire(dir.path()).expect("persistent unlocked file is vacant");
        assert_eq!(first.path(), path);
        drop(first);
        assert!(path.is_file(), "dropping a lease must not delete its inode");

        drop(try_acquire(dir.path()).expect("drop releases the kernel lock"));
        assert!(path.is_file(), "an unlocked lease file may persist forever");
    }

    #[test]
    fn separate_handles_in_one_process_observe_real_contention() {
        let dir = Tmp::new("same-process-contention");
        let first = try_acquire(dir.path()).expect("first lease");
        let second = try_acquire(dir.path()).expect_err("second independent handle is occupied");

        assert!(matches!(second, AcquireError::Occupied { .. }));
        assert!(second.to_string().contains("已被占用"));
        drop(first);
        drop(try_acquire(dir.path()).expect("released after owning handle drops"));
    }

    #[test]
    fn an_unusable_lock_path_is_unknown_not_occupied() {
        let dir = Tmp::new("unknown");
        let path = lock_path(dir.path());
        std::fs::create_dir(&path).expect("put a directory at recording.lock");

        let error = try_acquire(dir.path()).expect_err("directory cannot be opened as lock file");
        assert!(matches!(
            error,
            AcquireError::Unknown {
                operation: IoOperation::OpenLockFile,
                ..
            }
        ));
        assert!(error.to_string().contains("狀態不明"));
        assert!(error.to_string().contains(LOCK_FILE));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_at_lock_path_is_rejected_by_the_open_not_followed() {
        use std::os::unix::fs::symlink;

        let dir = Tmp::new("symlink");
        let target = dir.path().join("not-the-lease");
        std::fs::write(&target, b"target must never become the lock").expect("seed target");
        symlink(&target, lock_path(dir.path())).expect("create recording.lock symlink");

        let error = try_acquire(dir.path()).expect_err("O_NOFOLLOW must reject the symlink");
        assert!(matches!(
            error,
            AcquireError::Unknown {
                operation: IoOperation::OpenLockFile,
                ..
            }
        ));
        assert!(error.to_string().contains("狀態不明"));
        assert_eq!(
            std::fs::read(&target).expect("read untouched target"),
            b"target must never become the lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn opened_fifo_handle_is_unknown_because_it_is_not_regular() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let dir = Tmp::new("fifo");
        let path = lock_path(dir.path());
        let raw = CString::new(path.as_os_str().as_bytes()).expect("path has no nul");
        // SAFETY: `raw` is a live NUL-terminated pathname and the mode has no invalid bits.
        let result = unsafe { libc::mkfifo(raw.as_ptr(), 0o600) };
        assert_eq!(
            result,
            0,
            "create FIFO at recording.lock: {}",
            io::Error::last_os_error()
        );

        let error = try_acquire(dir.path()).expect_err("FIFO must not become a lease handle");
        assert!(matches!(
            error,
            AcquireError::Unknown {
                operation: IoOperation::InspectOpenedFile,
                ..
            }
        ));
        assert!(error.to_string().contains("驗證已開啟的 lease handle失敗"));
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_lock_never_grants_group_or_other_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = Tmp::new("private-mode");
        let lease = try_acquire(dir.path()).expect("create private lease file");
        let mode = lease
            ._file
            .metadata()
            .expect("opened lease metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "recording.lock must not grant group/other bits"
        );
    }

    /// 由下一個測試用 current test binary 精確挑出。一般 test run 沒帶 env 時直接回來。
    #[test]
    fn child_process_holds_lease() {
        let Some(data_dir) = std::env::var_os(CHILD_DATA_DIR) else {
            return;
        };
        let ready = std::env::var_os(CHILD_READY).expect("child ready path");
        let _lease = try_acquire(Path::new(&data_dir)).expect("child acquires lease");
        std::fs::write(ready, b"locked").expect("announce child lease");
        loop {
            std::thread::park();
        }
    }

    #[test]
    fn killed_child_releases_the_kernel_lease_but_not_the_file() {
        let dir = Tmp::new("killed-child");
        let ready = dir.path().join("child.ready");
        let child = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("recorder_lease::tests::child_process_holds_lease")
            .arg("--exact")
            .arg("--nocapture")
            .env(CHILD_DATA_DIR, dir.path())
            .env(CHILD_READY, &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn lease holder child");
        let mut child = ChildGuard(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.is_file() {
            if let Some(status) = child.0.try_wait().expect("probe child") {
                panic!("lease holder child exited before ready: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "lease holder child did not become ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(matches!(
            try_acquire(dir.path()),
            Err(AcquireError::Occupied { .. })
        ));
        child.0.kill().expect("kill holder without running Drop");
        let status = child.0.wait().expect("reap killed holder");
        assert!(!status.success(), "kill must be an abrupt process exit");

        assert!(
            lock_path(dir.path()).is_file(),
            "process death releases the lock, not the persistent file"
        );
        drop(try_acquire(dir.path()).expect("kernel releases lock when holder process dies"));
    }

    #[cfg(windows)]
    #[test]
    fn live_windows_lease_denies_path_deletion_until_the_handle_drops() {
        let dir = Tmp::new("deny-delete");
        let path = lock_path(dir.path());
        let lease = try_acquire(dir.path()).expect("acquire lease");

        std::fs::remove_file(&path)
            .expect_err("FILE_SHARE_DELETE must stay denied while this inode is the live lease");
        assert!(
            path.is_file(),
            "failed deletion must leave the lock pathname intact"
        );

        drop(lease);
        std::fs::remove_file(&path).expect("dropping the handle releases delete sharing");
    }
}
