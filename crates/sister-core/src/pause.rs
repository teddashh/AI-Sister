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
//! 檔案內容是暫停當下的毫秒時戳，純粹給 `doctor` 講人話用；判定先看檔案在不
//! 在，只有「在不在」本身讀不出來時才看資料目錄狀態。內容壞掉不影響正確性。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::dir_state::{DirState, dir_state};
use crate::model::Millis;

/// 她現在的暫停狀態，以及判成暫停的理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseState {
    /// 沒有暫停。
    Recording,
    /// 暫停中，而且知道從什麼時候起。
    Since(Millis),
    /// 暫停中：旗標檔確定在，但內容讀不出來。刪掉它可以解除。
    FlagPresentButUnreadable,
    /// 暫停中：data dir 是正常目錄，但旗標檔本身讀不到。
    FlagUncheckable,
    /// 暫停中：連 data dir 都讀不到，所以刻意一律當成暫停。
    /// 旗標在不在沒有看過，刪它不一定有用。
    PathUnreadable,
}

/// 旗標檔名。放在 data dir 裡，跟 `sister.db` 同一層。
const FLAG: &str = "paused.flag";

pub fn flag_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FLAG)
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
    // 子路徑讀不到，或 data dir 存在但不是目錄／狀態讀不到，都算暫停；只有確定
    // 是正常目錄而旗標不在，或 data dir 確定不存在，才算正在錄。
    !matches!(
        decide_for(data_dir, flag_path(data_dir).try_exists().map_err(|_| ())),
        PauseDecision::Recording
    )
}

/// 她現在的暫停狀態。
///
/// 判定和 [`is_paused`] 共用 [`decide_for`]，避免同一個問題各讀一次、得到兩個答案。
pub fn state(data_dir: &Path) -> PauseState {
    match decide_for(data_dir, flag_path(data_dir).try_exists().map_err(|_| ())) {
        PauseDecision::Recording => PauseState::Recording,
        PauseDecision::FlagPresent => paused_since(data_dir)
            .map(PauseState::Since)
            .unwrap_or(PauseState::FlagPresentButUnreadable),
        PauseDecision::FlagUncheckable => PauseState::FlagUncheckable,
        PauseDecision::PathUnreadable => PauseState::PathUnreadable,
    }
}

/// 從什麼時候開始暫停的。純顯示用；`None` 可能是旗標在但內容讀不出來、
/// 旗標本身無法檢查，也可能是連 data dir 都讀不到。要分辨這三種情況請用 [`state`]。
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
}
