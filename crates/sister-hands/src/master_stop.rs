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

/// **這一行沒有任何 Linux 測試守得住，別照 Linux 的綠燈改它。**
/// 理由整段寫在 [`crate::kill_switch::is_pulled`] 上，一字不改地適用於這裡：
/// 把它寫成 `switch_path(data_dir).try_exists().unwrap_or(true)` 在 Linux 上
/// 觀察不出差別（child 查詢要穿過 data dir 本人，所以一定先回 `Err`），
/// Windows 卻把同一個情境回成 `Ok(false)`，於是那種寫法會說「我確定開關不在」。
/// 走 `dir_state` 是為了讓 data dir 本人的狀態也進得了判斷。
pub fn is_stopped(data_dir: &Path) -> bool {
    decide_for(data_dir, switch_present(&switch_path(data_dir)))
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
    }?;
    // 寫完再問一次閘門自己看不看得到。**一個回報成功卻沒有停下來的全停，
    // 比一個大聲失敗的全停危險得多**——呼叫端會照著印「三層都停了」。
    // 上面每一條路都可能在某種檔案系統形狀下「成功」而閘門仍讀成沒停
    // （斷掉的 symlink 是實測過的一種），所以這裡不推理，直接量。
    if !is_stopped(data_dir) {
        return Err(std::io::Error::other(format!(
            "寫完 {} 之後，判斷閘門仍然讀成「沒有全停」；沒有停下任何一層",
            switch_path(data_dir).display()
        )));
    }
    Ok(())
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
        // 而且 engage 不可以安靜地回成功——呼叫端會照著印「三層都停了」。
        // 這條 symlink 之下沒有東西可寫，所以 engage 必須讓呼叫端知道。
        let engaged = engage(&dir, 1_000);
        assert!(
            engaged.is_ok() || is_stopped(&dir),
            "engage 回報成功卻沒有停：{engaged:?}"
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
