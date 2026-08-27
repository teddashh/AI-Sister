//! 拔手開關：兩個行程之間唯一的那條線。
//!
//! `sister do` 可能卡在等回答，桌面 tray 和另一個終端機仍要能立刻讓下一個動作
//! 交不出去。這個 repo 沒有獨立的 hands process，所以不能靠殺行程；一個放在
//! data dir 的檔案，是所有執行隘口都看得到、模型碰不到、行程當掉後仍保留的牆。
//!
//! 三條規則跟錄製暫停相同：不確定就是拔掉、不會自己過期、第一次拔掉的時間不能
//! 被重按洗掉。判定只看檔案在不在；內容只是顯示用，壞掉仍然算拔掉。

use std::path::{Path, PathBuf};

const SWITCH: &str = "hands.stop";

pub fn switch_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SWITCH)
}

/// data dir 本人的狀態。抽出來是因為 Windows 會把「父路徑是檔案」的子路徑
/// `try_exists` 回成 `Ok(false)`，Linux 卻回 `Err(NotADirectory)`；若只測 IO
/// 薄殼，Linux 永遠站不到 Windows 出貨時的那一格。
///
/// 這份判定刻意和 `sister-core::pause` 各留一份：兩個 crate 不能互相形成正常相依。
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

/// `child` 是 `data_dir/hands.stop` 的 `try_exists` 答案；錯誤內容在這一步不重要。
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
///
/// 把它改回 `switch_path(data_dir).try_exists().unwrap_or(true)`，Linux 上
/// 47 條測試全綠——實測過。原因是 child 查詢要穿過 data dir 本人，所以只要
/// data dir 不是可走的目錄，那個查詢在 Linux 上一定先回 `Err`，兩種寫法就
/// 觀察不出差別。Windows 不是這樣：它把同一個情境回成 `Ok(false)`，於是
/// 舊寫法會說「我確定開關不在」——fail-open，而且錯在最不該錯的方向。
///
/// 真正守住這一行的是 `unreadable_path_is_pulled_fail_closed`，而它只有在
/// Windows CI 上才走得到那一格（alpha.75 就是被它擋下來的）。下面那些
/// `decide` 的單元測試證明的是規則對，不是這一行有照著規則走。
pub fn is_pulled(data_dir: &Path) -> bool {
    decide_for(data_dir, switch_path(data_dir).try_exists().map_err(|_| ()))
}

pub fn pull(data_dir: &Path, at_ms: i64) -> std::io::Result<bool> {
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
        Ok(mut file) => {
            file.write_all(at_ms.to_string().as_bytes())?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn release(data_dir: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(switch_path(data_dir)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn pulled_since(data_dir: &Path) -> Option<i64> {
    std::fs::read_to_string(switch_path(data_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn tray_label(data_dir: &Path) -> String {
    if !is_pulled(data_dir) {
        return "拔掉她的手".into();
    }
    match pulled_since(data_dir) {
        Some(since) => format!("把手接回去（從 {} 拔的）", crate::replay_copy::at(since)),
        None => "把手接回去（拔手時間讀不到）".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sister-hands-switch-{}-{name}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// **這是整個 repo 唯一真的走過 `is_pulled` 的 Windows 那一格的測試。**
    /// 在 Linux 它從 `Err(NotADirectory)` 那條路過關，證不了什麼；在 Windows
    /// 它從 `Ok(false)` 那條路過關，而那正是 alpha.75 之前會 fail-open 的格子
    /// ——這條測試就是在 Windows CI 上把那個 bug 擋下來的人。別因為下面的
    /// `decide` 單元測試看起來涵蓋一樣的規則就刪掉它，那些測試碰不到薄殼。
    #[test]
    fn unreadable_path_is_pulled_fail_closed() {
        let tmp = Tmp::new("fail-closed");
        std::fs::remove_dir_all(&tmp.0).unwrap();
        std::fs::write(&tmp.0, "not a directory").unwrap();
        assert!(is_pulled(&tmp.0));
    }

    #[test]
    fn windows_child_missing_with_non_directory_parent_is_fail_closed() {
        // Linux 的 child `try_exists` 會先回 Err，跑不出 Windows 的 Ok(false) 組合。
        assert!(decide(Ok(false), DirState::NotADir));
    }

    #[test]
    fn absent_data_dir_is_not_pulled() {
        assert!(!decide(Ok(false), DirState::Absent));
    }

    #[test]
    fn directory_without_switch_is_not_pulled() {
        assert!(!decide(Ok(false), DirState::Dir));
    }

    #[test]
    fn present_switch_and_unreadable_child_are_pulled() {
        assert!(decide(Ok(true), DirState::NotADir));
        assert!(decide(Err(()), DirState::Dir));
    }

    #[test]
    fn shell_maps_real_paths_to_the_right_states() {
        let tmp = Tmp::new("shell-states");
        assert!(!is_pulled(&tmp.0));

        let absent = tmp.0.join("absent");
        assert!(!is_pulled(&absent));

        let file = tmp.0.join("file");
        std::fs::write(&file, "not a directory").unwrap();
        assert!(is_pulled(&file));
        // Windows 對這個真實路徑的 child 查詢回 Ok(false)；Linux 無法自然產生，
        // 所以在 IO 邊界明確餵入該答案，並仍由薄殼讀取真實 data dir 狀態。
        assert!(decide_for(&file, Ok(false)));
    }

    #[test]
    fn pulling_twice_does_not_replace_the_first_timestamp() {
        let tmp = Tmp::new("idempotent");
        assert!(pull(&tmp.0, 1000).unwrap());
        assert!(!pull(&tmp.0, 9999).unwrap());
        assert_eq!(pulled_since(&tmp.0), Some(1000));
    }

    #[test]
    fn release_distinguishes_removed_from_already_attached() {
        let tmp = Tmp::new("release");
        pull(&tmp.0, 1000).unwrap();
        assert!(release(&tmp.0).unwrap());
        assert!(!release(&tmp.0).unwrap());
    }
}
