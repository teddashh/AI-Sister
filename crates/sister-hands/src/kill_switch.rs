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

pub fn is_pulled(data_dir: &Path) -> bool {
    switch_path(data_dir).try_exists().unwrap_or(true)
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

    #[test]
    fn unreadable_path_is_pulled_fail_closed() {
        let tmp = Tmp::new("fail-closed");
        std::fs::remove_dir_all(&tmp.0).unwrap();
        std::fs::write(&tmp.0, "not a directory").unwrap();
        assert!(is_pulled(&tmp.0));
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
