//! 兩個呼叫端共用的平台執行層：字母人和 `sister do`。
//!
//! 「她被允許碰什麼」留兩份拷貝，正是這個 repo 一路在修的錯；而放在
//! `apps/desktop` 的程式在 CI 只會被 clippy/build，不會有一列被 `cargo test`
//! 執行。`target_policy` 當初搬進 `sister-hands` 也是同一個理由。

use crate::{Attached, Executor, Suggestion, kill_switch};
use std::path::{Path, PathBuf};

pub struct PlatformExecutor {
    data_dir: PathBuf,
}

impl PlatformExecutor {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }
}

impl Executor for PlatformExecutor {
    fn execute(&mut self, suggestion: &Suggestion) -> Result<String, String> {
        // **第二道，不是重複。** 上面那一道在 `execute_with` /
        // `execute_approved_step` 裡，負責講出好聽得懂的拒絕理由；這一道貼著
        // 系統呼叫，負責的是「就算有人之後把 `hands_attached` 寫成永遠回 Yes，
        // 這裡還是交不出去」。兩道之間是 TOCTOU 窗口，這一道把它縮到只剩
        // `platform_execute` 裡的 ShellExecuteW 本身。走到這裡代表上面那一道
        // 已被繞過；此時寧可讓結果文字落成 Failed，也不能交出去。
        if kill_switch::is_pulled(&self.data_dir) {
            return Err("手被拔掉了，沒有交給作業系統。".into());
        }
        platform_execute(suggestion)
    }

    fn hands_attached(&self) -> Attached {
        if kill_switch::is_pulled(&self.data_dir) {
            Attached::No {
                since_ms: kill_switch::pulled_since(&self.data_dir),
            }
        } else {
            Attached::Yes
        }
    }
}

#[cfg(not(windows))]
fn platform_execute(_suggestion: &Suggestion) -> Result<String, String> {
    Err("這台機器上做不到：這一版只有 Windows 平台執行層".into())
}

#[cfg(windows)]
fn platform_execute(suggestion: &Suggestion) -> Result<String, String> {
    use crate::target_policy::{validate_file, validate_url, validate_window_title};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible, SW_SHOWNORMAL,
        SetForegroundWindow,
    };
    use windows::core::{BOOL, PCWSTR};

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        value.encode_wide().chain(Some(0)).collect()
    }
    fn shell_open(value: &std::ffi::OsStr) -> Result<String, String> {
        let value = wide(value);
        let verb = wide(std::ffi::OsStr::new("open"));
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb.as_ptr()),
                PCWSTR(value.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            Err(format!(
                "作業系統拒絕開啟（ShellExecuteW={}）",
                result.0 as isize
            ))
        } else {
            Ok("作業系統已接受開啟請求".into())
        }
    }
    unsafe extern "system" fn find(hwnd: HWND, raw: LPARAM) -> BOOL {
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return BOOL(1);
        }
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len <= 0 {
            return BOOL(1);
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = unsafe { GetWindowTextW(hwnd, &mut buf) };
        let state = unsafe { &mut *(raw.0 as *mut (&str, Option<HWND>)) };
        if String::from_utf16_lossy(&buf[..got as usize]).contains(state.0) {
            state.1 = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
    match suggestion {
        Suggestion::OpenUrl { url, .. } => {
            validate_url(url)?;
            shell_open(std::ffi::OsStr::new(url))
        }
        Suggestion::OpenFile { path, .. } => {
            validate_file(path)?;
            shell_open(path.as_os_str())
        }
        Suggestion::FocusWindow { title, .. } => {
            validate_window_title(title)?;
            let mut state = (title.as_str(), None);
            unsafe {
                let _ = EnumWindows(Some(find), LPARAM(&mut state as *mut _ as isize));
            }
            let hwnd = state
                .1
                .ok_or_else(|| format!("找不到標題含「{title}」的視窗"))?;
            if unsafe { SetForegroundWindow(hwnd) }.as_bool() {
                Ok(format!("已聚焦視窗：{title}"))
            } else {
                Err(format!("Windows 不允許聚焦視窗：{title}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_executor_reads_the_real_kill_switch() {
        let dir =
            std::env::temp_dir().join(format!("sister-platform-switch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let executor = PlatformExecutor::new(&dir);
        assert_eq!(executor.hands_attached(), Attached::Yes);
        assert!(kill_switch::pull(&dir, 1000).unwrap());
        assert_eq!(
            executor.hands_attached(),
            Attached::No {
                since_ms: Some(1000)
            }
        );
        assert!(kill_switch::release(&dir).unwrap());
        assert_eq!(executor.hands_attached(), Attached::Yes);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
