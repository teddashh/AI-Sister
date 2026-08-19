//! 字母人對 recorder 說的話。[`crate::heartbeat`] 是反方向。
//!
//! 「請你停下來」為什麼是一個檔案，而不是去 kill 那個行程：
//!
//! `TerminateProcess` 會讓 recorder 死在半路——`end_session` 不會被寫、心跳
//! 檔留在磁碟上（於是接下來 16 秒字母人會說她還在錄）、正在寫的那張圖變成
//! 半個檔案。一個「乾淨地收工」的請求，必須由 recorder 自己在它知道自己在
//! 哪裡的時候執行。
//!
//! 而且這條路和暫停、心跳是同一個形狀：**沒有常駐服務、沒有 port、沒有 IPC**，
//! 一個檔案在不在就是全部的協定。少一個 port 就少一個「有人搶走那個 port
//! 之後會怎樣」的問題。
//!
//! 停止請求和暫停是兩件事，不要合併：暫停是「先別看，但留在這裡」，停止是
//! 「今天到此為止」。把停止做成「一直暫停」會留下一個永遠在跑、卻永遠不做
//! 事的行程，而使用者在工作管理員裡看得到它——那看起來就像她賴著不走。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 檔名。放在 data dir 裡，跟 `sister.db` 同一層。
const STOP: &str = "stop.request";

pub fn stop_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STOP)
}

/// 請正在跑的那個 recorder 收工。
///
/// 沒有人在跑也不算錯：檔案就留在那裡，而下一個起來的 recorder 會**先把它
/// 清掉再開始**（見 [`take_stop`] 的呼叫端）。少了那一步，這個檔案會變成一顆
/// 地雷——下次按「開始記錄」的時候她會立刻自己停掉，而沒有人看得出為什麼。
pub fn request_stop(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let path = stop_path(data_dir);
    std::fs::write(&path, "stop").with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// 有沒有人叫我停？**問到就順手拿走**。
///
/// 一次消費掉是刻意的：留著的話，recorder 下一個 tick 會再讀到同一個請求。
/// 這裡回 `bool` 而不是 `Result`，理由和 [`crate::pause::is_paused`] 一樣——
/// 只有一個安全的預設答案。刪不掉（權限、檔案被鎖）仍然回 `true`：那個請求
/// 是真的，該停還是要停；壞掉的是清理，不是意圖。
pub fn take_stop(data_dir: &Path) -> bool {
    let path = stop_path(data_dir);
    match path.try_exists() {
        Ok(true) => {
            let _ = std::fs::remove_file(&path);
            true
        }
        // 讀不到就當作沒有人叫停。這裡和暫停刻意相反：暫停「不確定就是暫停」
        // 是為了少錄，而一個問不出來就自己停掉的 recorder，會在檔案系統打嗝
        // 的時候安靜地結束一整天的記錄。
        _ => false,
    }
}

/// 開始之前先把舊的請求清掉。見 [`request_stop`] 的說明。
pub fn clear_stop(data_dir: &Path) {
    let _ = std::fs::remove_file(stop_path(data_dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("sister-ctl-{name}-{}", std::process::id()));
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
    fn nobody_asked_means_keep_going() {
        let t = Tmp::new("none");
        assert!(!take_stop(&t.0));
    }

    /// 拿走一次就沒了。留著的話 recorder 下一個 tick 會再停一次——而它
    /// 已經停了，所以真正的症狀是**下一場**錄製一開始就自己結束。
    #[test]
    fn a_stop_request_is_consumed_once() {
        let t = Tmp::new("once");
        request_stop(&t.0).expect("request");
        assert!(take_stop(&t.0));
        assert!(!take_stop(&t.0), "同一個請求不可以停第二次");
    }

    /// 沒有人在跑的時候按下「停止」，檔案會留著。下一場開始前必須清掉，
    /// 否則她會在啟動的瞬間自己停掉，而畫面上看不出任何原因。
    #[test]
    fn a_leftover_request_does_not_kill_the_next_run() {
        let t = Tmp::new("leftover");
        request_stop(&t.0).expect("request");
        clear_stop(&t.0);
        assert!(!take_stop(&t.0));
    }

    /// 停止和暫停是兩個檔案、兩件事。停止請求不可以把暫停狀態洗掉——
    /// 「我昨天按了暫停」在下一場錄製裡仍然要成立。
    #[test]
    fn stopping_does_not_touch_the_pause_flag() {
        let t = Tmp::new("orthogonal");
        crate::pause::set_paused(&t.0, true, 1_000).expect("pause");
        request_stop(&t.0).expect("request");
        assert!(take_stop(&t.0));
        assert!(crate::pause::is_paused(&t.0), "暫停旗標要原封不動");
    }
}
