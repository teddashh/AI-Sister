//! 暫停旗標：兩個行程之間唯一的那條線。
//!
//! `sister.exe record` 在錄，`sister-desktop.exe` 上有一顆暫停鍵，兩者是**不同
//! 的行程**——沒有常駐服務、沒有 port、沒有 IPC。一個檔案在不在，是它們唯一
//! 都看得到而且不必先發明一套協定的東西，而且它在任何一邊當掉之後還在。
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
//! 檔案內容是暫停當下的毫秒時戳，純粹給 `doctor` 講人話用；**判定只看檔案在不
//! 在**。內容壞掉不影響正確性。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::model::Millis;

/// 旗標檔名。放在 data dir 裡，跟 `sister.db` 同一層。
const FLAG: &str = "paused.flag";

pub fn flag_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FLAG)
}

/// 她現在是不是閉著眼睛。
///
/// 回傳 `bool` 而不是 `Result<bool>` 是刻意的：這個問題只有一個安全的預設答案，
/// 而把錯誤丟給呼叫端，等於讓每一個呼叫端各自決定一次「讀不到的時候要不要繼續
/// 錄」——只要有一個人答錯，承諾就破了。所以答案在這裡就定死。
pub fn is_paused(data_dir: &Path) -> bool {
    // 權限不足、路徑中間有個檔案不是資料夾、IO 出錯……都算暫停。
    flag_path(data_dir).try_exists().unwrap_or(true)
}

/// 從什麼時候開始暫停的。純顯示用；`None` 代表旗標在但內容讀不出來
/// （例如寫到一半斷電），那仍然是暫停。
pub fn paused_since(data_dir: &Path) -> Option<Millis> {
    std::fs::read_to_string(flag_path(data_dir))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// 按下暫停／解除暫停。
///
/// 兩個方向都是幂等的：已經暫停了再按暫停不會動到原本的時戳（不然「暫停多久
/// 了」會被每一次重按洗掉），已經在錄了再按解除也不算錯誤。
pub fn set_paused(data_dir: &Path, paused: bool, ts: Millis) -> Result<()> {
    let path = flag_path(data_dir);
    if paused {
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("建立 {} 失敗", parent.display()))?;
        }
        std::fs::write(&path, ts.to_string())
            .with_context(|| format!("寫入 {} 失敗", path.display()))?;
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("刪除 {} 失敗", path.display()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

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
        }
    }

    #[test]
    fn a_fresh_machine_is_recording() {
        let dir = Tmp::new("fresh");
        assert!(!is_paused(dir.path()));
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
}
