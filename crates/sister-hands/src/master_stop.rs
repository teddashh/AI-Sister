//! 全停開關放在 `sister-hands`，因為它是三層共用依賴的最底層。
//!
//! `sister-core` 與 `sister-capture` 都已經向下依賴 hands，反過來卻不成立；把這條
//! 跨 capture／brain／hands 的 durable latch 放在任何更高層，都會形成反向相依或
//! 留下一層讀不到。data dir 裡的一個檔案讓不同程序看見同一個狀態，行程重開後仍保留。
//!
//! 三條規則和拔手開關相同：不確定就是停止、不會自己過期、第一次停止的時間不能
//! 被重按洗掉。判定只看檔案在不在；內容只是顯示用，壞掉仍然算停止。

use std::path::{Path, PathBuf};

const SWITCH: &str = "master.stop";

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

fn dir_state(data_dir: &Path) -> DirState {
    match std::fs::metadata(data_dir) {
        Ok(metadata) if metadata.is_dir() => DirState::Dir,
        Ok(_) => DirState::NotADir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DirState::Absent,
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

/// **這一行沒有任何 Linux 測試守得住，別照 Linux 的綠燈改它。**
/// 理由整段寫在 [`crate::kill_switch::is_pulled`] 上，一字不改地適用於這裡：
/// 把它寫成 `switch_path(data_dir).try_exists().unwrap_or(true)` 在 Linux 上
/// 觀察不出差別（child 查詢要穿過 data dir 本人，所以一定先回 `Err`），
/// Windows 卻把同一個情境回成 `Ok(false)`，於是那種寫法會說「我確定開關不在」。
/// 走 `dir_state` 是為了讓 data dir 本人的狀態也進得了判斷。
pub fn is_stopped(data_dir: &Path) -> bool {
    decide_for(data_dir, switch_path(data_dir).try_exists().map_err(|_| ()))
}

pub fn stopped_since(data_dir: &Path) -> Option<i64> {
    std::fs::read_to_string(switch_path(data_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn engage(data_dir: &Path, now_ms: i64) -> std::io::Result<()> {
    let path = switch_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(now_ms.to_string().as_bytes()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn release(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(switch_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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
