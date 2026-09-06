//! 兩個呼叫端共用的平台執行層：字母人和 `sister do`。
//!
//! 「她被允許碰什麼」留兩份拷貝，正是這個 repo 一路在修的錯；而放在
//! `apps/desktop` 是獨立 workspace：根 workspace 的 Linux 測試碰不到它，
//! alpha.100 起 Windows CI 才另外執行它的 unit tests。`target_policy` 搬進
//! `sister-hands`，是為了讓 CLI 和字母人共用同一份規則，並讓 Linux CI 也跑得到。

use crate::{Attached, Executor, ExecutorError, RefusalReason, Suggestion, kill_switch};
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
    fn execute(&mut self, suggestion: &Suggestion) -> Result<String, ExecutorError> {
        // **第二道，不是重複。** 上面那一道在 `execute_with` /
        // `execute_approved_step` 裡，負責講出好聽得懂的拒絕理由；這一道貼著
        // 系統呼叫，負責的是「就算有人之後把 `hands_attached` 寫成永遠回 Yes，
        // 這裡還是交不出去」。兩道之間是 TOCTOU 窗口，這一道把它縮到只剩
        // `platform_execute` 裡的 ShellExecuteW 本身。走到這裡代表上面那一道
        // 已被繞過；typed `ExecutorError` 會讓它仍落成 Refused，不會謊稱碰過 OS。
        if kill_switch::is_pulled(&self.data_dir) {
            return Err(ExecutorError::refused(RefusalReason::HandsPulled {
                since_ms: kill_switch::pulled_since(&self.data_dir),
            }));
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

/// 驗證留在平台分支**外面**，讓 Linux CI 也真的跑得到貼著 OS 呼叫的這一道。
/// Windows 分支裡再驗一次不會多一層保護，只會留下一份 Linux 永遠碰不到的規則。
fn platform_execute(suggestion: &Suggestion) -> Result<String, ExecutorError> {
    crate::target_policy::validate_suggestion(suggestion)
        .map_err(|why| ExecutorError::refused(RefusalReason::TargetRejectedBeforeOs { why }))?;
    platform_execute_validated(suggestion).map_err(ExecutorError::platform)
}

#[cfg(not(windows))]
fn platform_execute_validated(_suggestion: &Suggestion) -> Result<String, String> {
    Err("這台機器上做不到：這一版只有 Windows 平台執行層".into())
}

#[cfg(windows)]
fn platform_execute_validated(suggestion: &Suggestion) -> Result<String, String> {
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
        Suggestion::OpenUrl { url, .. } => shell_open(std::ffi::OsStr::new(url)),
        Suggestion::OpenFile { path, .. } => shell_open(path.as_os_str()),
        Suggestion::FocusWindow { title, .. } => {
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
    use crate::SuggestionButton;

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

    /// `ShellExecuteW` 會把這個「檔案」交給瀏覽器。這一條必須在 Linux CI 也走過
    /// 和 Windows 相同的最後一道 target validation；只斷言 `Err` 會把 Linux 的
    /// 「平台不支援」誤當成綠，所以還要釘住真正的拒絕理由。
    #[test]
    fn a_disguised_file_target_is_stopped_before_the_platform_call() {
        for (json, expected) in [
            (
                r#"{"action":"open_file","path":"https://evil.example/report.pdf"}"#,
                "不能是 URI",
            ),
            (
                r#"{"action":"open_file","path":"C:\\work\\evil.exe\u0000.pdf"}"#,
                "控制字元",
            ),
        ] {
            let suggestion = SuggestionButton::parse_json(json)
                .expect("syntactically valid suggestion")
                .press();
            let error = platform_execute(&suggestion).expect_err("not a safe file target");
            let ExecutorError::RefusedBeforeOs {
                reason: RefusalReason::TargetRejectedBeforeOs { why },
            } = error
            else {
                panic!("{json}: 目標被擋卻不是 pre-OS refusal：{error:?}");
            };
            assert!(why.contains(expected), "{json}: {why}");
            assert!(!why.contains("這台機器上做不到"), "{json}: {why}");
        }
    }
}
